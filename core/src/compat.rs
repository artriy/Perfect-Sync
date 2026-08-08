//! Cross-platform launch + loader compatibility.
//!
//! Among Us is a Windows-only Unity build, so the BepInEx Doorstop files we
//! install are always the Windows pack. What changes per host is how the game
//! runs: native on Windows, through Steam Proton on Linux, or through a
//! Wine-based bottle manager. Runtime classification is separate from launcher
//! discovery so every host branch can be tested on every build machine.

use crate::process::{LaunchSpec, GAME_EXE};
use crate::types::Runtime;
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Among Us Steam app id (its Proton prefix lives at `compatdata/<id>/pfx`).
pub const STEAM_APP_ID: &str = "945360";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostPlatform {
    Windows,
    Linux,
    Macos,
    Other,
}

pub const fn current_host() -> HostPlatform {
    if cfg!(target_os = "windows") {
        HostPlatform::Windows
    } else if cfg!(target_os = "linux") {
        HostPlatform::Linux
    } else if cfg!(target_os = "macos") {
        HostPlatform::Macos
    } else {
        HostPlatform::Other
    }
}

/// How (and where) a given game dir should be launched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeContext {
    pub host: HostPlatform,
    pub runtime: Runtime,
    /// Wine prefix (the dir holding `user.reg`): Proton's `compatdata/<id>/pfx`
    /// or a Wine/CrossOver/Whisky/Bottles bottle. `None` on native Windows.
    pub prefix: Option<PathBuf>,
    /// Binary used to start the selected executable (`proton`, `wine`, etc.).
    /// `None` means runtime discovery failed.
    pub launcher: Option<PathBuf>,
    /// Arguments which select the launcher/bottle, before the game-specific args.
    pub launcher_args: Vec<OsString>,
}

fn normalized(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/").to_lowercase()
}

fn is_bottle_runtime(runtime: Runtime) -> bool {
    matches!(
        runtime,
        Runtime::Wine | Runtime::Crossover | Runtime::Whisky | Runtime::Bottles
    )
}

fn runtime_from_bottle_path(path: &Path) -> Runtime {
    let p = normalized(path);
    if p.contains("/crossover/bottles/") {
        Runtime::Crossover
    } else if p.contains("com.isaacmarovitz.whisky") || p.contains("/whisky/bottles/") {
        Runtime::Whisky
    } else if p.contains("com.usebottles.bottles") || p.contains("/bottles/bottles/") {
        Runtime::Bottles
    } else {
        Runtime::Wine
    }
}

/// Classify a path without probing installed launcher binaries. `hint` is the
/// runtime persisted when the user picked an auto-detected install; structural
/// bottle detection still wins over the generic `steamapps` marker.
pub fn classify_runtime(
    game_dir: &Path,
    host: HostPlatform,
    hint: Option<Runtime>,
) -> (Runtime, Option<PathBuf>) {
    if host == HostPlatform::Windows {
        return (Runtime::Native, None);
    }

    // Canonicalization follows Wine's `dosdevices/c:` links when available.
    // Nonexistent paths (including pure test fixtures) keep their original form.
    let canonical = fs::canonicalize(game_dir).ok();
    let structural = canonical.as_deref().unwrap_or(game_dir);
    if let Some(prefix) = wine_prefix_from_game(structural) {
        let detected = runtime_from_bottle_path(structural);
        let runtime = hint.filter(|r| is_bottle_runtime(*r)).unwrap_or(detected);
        return (runtime, Some(prefix));
    }

    let p = normalized(structural);
    if host == HostPlatform::Linux && p.contains("/steamapps/") {
        return (Runtime::Proton, proton_prefix_from_game(structural));
    }

    // macOS has no Steam Proton. A manually selected Windows game outside a
    // recognizable bottle is treated as Wine and produces actionable guidance
    // if no Wine launcher can be found.
    let runtime = hint
        .filter(|r| is_bottle_runtime(*r))
        .unwrap_or(Runtime::Wine);
    (runtime, None)
}

/// Resolve the current host with no persisted runtime hint.
pub fn resolve(game_dir: &Path) -> RuntimeContext {
    resolve_with_hint(game_dir, None)
}

/// Resolve the current host, honoring a runtime saved for this exact game path.
pub fn resolve_with_hint(game_dir: &Path, hint: Option<Runtime>) -> RuntimeContext {
    resolve_for_host(game_dir, current_host(), hint)
}

/// Host-injectable resolver used by the real entrypoint and cross-platform tests.
pub fn resolve_for_host(
    game_dir: &Path,
    host: HostPlatform,
    hint: Option<Runtime>,
) -> RuntimeContext {
    let (runtime, mut prefix) = classify_runtime(game_dir, host, hint);
    if runtime == Runtime::Wine && prefix.is_none() {
        prefix = default_wine_prefix();
    }
    let (launcher, launcher_args) = find_launcher(game_dir, runtime, prefix.as_deref());
    RuntimeContext {
        host,
        runtime,
        prefix,
        launcher,
        launcher_args,
    }
}

/// Proton prefix for a Steam game dir:
/// `<lib>/steamapps/common/<game>` -> `<lib>/steamapps/compatdata/945360/pfx`.
pub fn proton_prefix_from_game(game_dir: &Path) -> Option<PathBuf> {
    for anc in game_dir.ancestors() {
        if anc.file_name().and_then(|n| n.to_str()) == Some("steamapps") {
            return Some(anc.join("compatdata").join(STEAM_APP_ID).join("pfx"));
        }
    }
    None
}
fn steam_root_from_prefix(prefix: &Path) -> Option<PathBuf> {
    prefix
        .ancestors()
        .find(|ancestor| ancestor.file_name().and_then(|name| name.to_str()) == Some("steamapps"))
        .and_then(Path::parent)
        .map(Path::to_path_buf)
}

/// Wine prefix (the dir containing `drive_c`) for a game dir inside a bottle.
pub fn wine_prefix_from_game(game_dir: &Path) -> Option<PathBuf> {
    for anc in game_dir.ancestors() {
        if anc.file_name().and_then(|n| n.to_str()) == Some("drive_c") {
            return anc.parent().map(Path::to_path_buf);
        }
    }
    None
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

fn default_wine_prefix() -> Option<PathBuf> {
    std::env::var_os("WINEPREFIX")
        .map(PathBuf::from)
        .or_else(|| home_dir().map(|home| home.join(".wine")))
}

/// First existing binary across PATH plus common GUI-app-safe system locations.
fn find_binary(names: &[&str]) -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            for name in names {
                let candidate = dir.join(name);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    for dir in ["/usr/bin", "/usr/local/bin", "/opt/homebrew/bin"] {
        for name in names {
            let candidate = Path::new(dir).join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

fn find_flatpak() -> Option<PathBuf> {
    find_binary(&["flatpak"])
}

fn proton_launcher(game_dir: &Path, prefix: Option<&Path>) -> (Option<PathBuf>, Vec<OsString>) {
    let mut roots = Vec::new();
    if let Some(paths) = std::env::var_os("STEAM_COMPAT_TOOL_PATHS") {
        roots.extend(std::env::split_paths(&paths));
    }
    if let Some(steamapps) = game_dir
        .ancestors()
        .find(|ancestor| ancestor.file_name().and_then(|name| name.to_str()) == Some("steamapps"))
    {
        roots.push(steamapps.join("common"));
        if let Some(steam_root) = steamapps.parent() {
            roots.push(steam_root.join("compatibilitytools.d"));
        }
    }
    if let Some(home) = home_dir() {
        roots.push(home.join(".steam/root/compatibilitytools.d"));
        roots.push(home.join(".local/share/Steam/compatibilitytools.d"));
        roots.push(home.join(".var/app/com.valvesoftware.Steam/data/Steam/compatibilitytools.d"));
    }

    let mut candidates = Vec::new();
    for root in roots {
        let direct = root.join("proton");
        if direct.is_file() {
            candidates.push(direct);
            continue;
        }
        let Ok(entries) = fs::read_dir(&root) else {
            continue;
        };
        for entry in entries.flatten().take(128) {
            let candidate = entry.path().join("proton");
            if candidate.is_file() {
                candidates.push(candidate);
            }
        }
    }
    candidates.sort();
    candidates.dedup();
    let desired = prefix
        .and_then(Path::parent)
        .and_then(|compatdata| fs::read_to_string(compatdata.join("version")).ok())
        .or_else(|| {
            prefix
                .and_then(Path::parent)
                .and_then(|compatdata| fs::read_to_string(compatdata.join("config_info")).ok())
        })
        .and_then(|value| value.lines().next().map(str::to_owned));
    candidates.sort_by_key(|candidate| {
        let version_matches = desired.as_ref().is_some_and(|desired| {
            candidate
                .parent()
                .and_then(|root| fs::read_to_string(root.join("version")).ok())
                .is_some_and(|version| {
                    version.contains(desired) || desired.contains(version.trim())
                })
        });
        let modified = fs::metadata(candidate)
            .and_then(|metadata| metadata.modified())
            .ok();
        (version_matches, modified)
    });
    (candidates.pop(), vec![OsString::from("run")])
}

const CROSSOVER_WINE_RELATIVE_PATH: &str = "Contents/SharedSupport/CrossOver/bin/wine";

fn crossover_wine_from_root(root: &Path) -> Option<PathBuf> {
    [
        root.join("bin/wine"),
        root.join(CROSSOVER_WINE_RELATIVE_PATH),
    ]
    .into_iter()
    .find(|candidate| candidate.is_file())
}

fn find_crossover_wine_in_applications(applications: &Path) -> Option<PathBuf> {
    let mut apps = fs::read_dir(applications)
        .ok()?
        .flatten()
        .take(256)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("CrossOver") && name.ends_with(".app"))
        })
        .collect::<Vec<_>>();
    apps.sort();
    apps.into_iter()
        .find_map(|application| crossover_wine_from_root(&application))
}

fn find_crossover_wine_on_path() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|directory| directory.join("wine"))
        .find(|candidate| {
            candidate.is_file()
                && candidate
                    .parent()
                    .and_then(Path::parent)
                    .is_some_and(|root| root.join("etc/CrossOver.conf").is_file())
        })
}

fn find_crossover_wine() -> Option<PathBuf> {
    if let Some(wine) = std::env::var_os("CX_ROOT")
        .map(PathBuf::from)
        .and_then(|root| crossover_wine_from_root(&root))
    {
        return Some(wine);
    }

    let mut applications = vec![PathBuf::from("/Applications")];
    let mut direct = vec![PathBuf::from("/Applications/CrossOver.app")];
    if let Some(home) = home_dir() {
        direct.push(home.join("CrossOver.app"));
        direct.push(home.join("Applications/CrossOver.app"));
        applications.push(home.join("Applications"));
    }
    direct
        .into_iter()
        .find_map(|application| crossover_wine_from_root(&application))
        .or_else(|| {
            applications
                .into_iter()
                .find_map(|root| find_crossover_wine_in_applications(&root))
        })
        .or_else(find_crossover_wine_on_path)
}

fn find_whisky_wine() -> Option<PathBuf> {
    let home = home_dir()?;
    [
        home.join("Library/Containers/com.isaacmarovitz.Whisky/Data/Library/Application Support/com.isaacmarovitz.Whisky/Libraries/Wine/bin/wine64"),
        home.join("Library/Application Support/com.isaacmarovitz.Whisky/Libraries/Wine/bin/wine64"),
    ]
    .into_iter()
    .find(|p| p.is_file())
}

fn bottle_name(prefix: Option<&Path>) -> Option<OsString> {
    prefix?.file_name().map(OsString::from)
}

fn find_launcher(
    game_dir: &Path,
    runtime: Runtime,
    prefix: Option<&Path>,
) -> (Option<PathBuf>, Vec<OsString>) {
    match runtime {
        Runtime::Native => (None, Vec::new()),
        Runtime::Proton => proton_launcher(game_dir, prefix),
        Runtime::Wine => (find_binary(&["wine", "wine64"]), Vec::new()),
        Runtime::Crossover => {
            let mut args = Vec::new();
            if let Some(name) = bottle_name(prefix) {
                args.extend([OsString::from("--bottle"), name, OsString::from("--")]);
            }
            (find_crossover_wine(), args)
        }
        Runtime::Whisky => (find_whisky_wine(), vec!["start".into(), "/unix".into()]),
        Runtime::Bottles => {
            let name = bottle_name(prefix).unwrap_or_else(|| OsString::from("Among Us"));
            let flatpak_install = normalized(game_dir).contains("com.usebottles.bottles");
            if flatpak_install {
                if let Some(flatpak) = find_flatpak() {
                    return (
                        Some(flatpak),
                        vec![
                            "run".into(),
                            "--command=bottles-cli".into(),
                            "com.usebottles.bottles".into(),
                            "run".into(),
                            "-b".into(),
                            name,
                            "-e".into(),
                        ],
                    );
                }
            }
            if let Some(cli) = find_binary(&["bottles-cli"]) {
                return (
                    Some(cli),
                    vec!["run".into(), "-b".into(), name, "-e".into()],
                );
            }
            if let Some(flatpak) = find_flatpak() {
                return (
                    Some(flatpak),
                    vec![
                        "run".into(),
                        "--command=bottles-cli".into(),
                        "com.usebottles.bottles".into(),
                        "run".into(),
                        "-b".into(),
                        name,
                        "-e".into(),
                    ],
                );
            }
            (None, vec!["run".into(), "-b".into(), name, "-e".into()])
        }
    }
}

fn runtime_fallback(runtime: Runtime) -> &'static str {
    match runtime {
        Runtime::Native => GAME_EXE,
        Runtime::Proton => "proton",
        Runtime::Wine => "wine",
        Runtime::Crossover => "wine",
        Runtime::Whisky => "wine64",
        Runtime::Bottles => "bottles-cli",
    }
}

fn wine_environment(
    ctx: &RuntimeContext,
    inherited_override: Option<OsString>,
) -> Vec<(OsString, OsString)> {
    let mut env = Vec::new();
    if inherited_override.is_none() {
        env.push(("WINEDLLOVERRIDES".into(), "winhttp=n,b".into()));
    }
    if let Some(prefix) = &ctx.prefix {
        if ctx.runtime == Runtime::Crossover {
            if let Some(name) = bottle_name(Some(prefix)) {
                env.push(("CX_BOTTLE".into(), name));
            }
        } else if !matches!(ctx.runtime, Runtime::Bottles | Runtime::Proton) {
            env.push(("WINEPREFIX".into(), prefix.as_os_str().to_owned()));
        }
    }
    if ctx.runtime == Runtime::Proton {
        if let Some(compatdata) = ctx.prefix.as_deref().and_then(Path::parent) {
            env.push((
                "STEAM_COMPAT_DATA_PATH".into(),
                compatdata.as_os_str().to_owned(),
            ));
        }
        if let Some(steam_root) = ctx.prefix.as_deref().and_then(steam_root_from_prefix) {
            env.push((
                "STEAM_COMPAT_CLIENT_INSTALL_PATH".into(),
                steam_root.as_os_str().to_owned(),
            ));
        }
        env.extend([
            ("SteamAppId".into(), STEAM_APP_ID.into()),
            ("SteamGameId".into(), STEAM_APP_ID.into()),
            ("STEAM_COMPAT_APP_ID".into(), STEAM_APP_ID.into()),
        ]);
    }
    env
}

const CROSSOVER_BUILTIN_GRAPHICS_OVERRIDE: &str = "d3d11,dxgi=b";
const CROSSOVER_WINED3D_ENV: &str = "CX_GRAPHICS_BACKEND=wined3d";

fn crossover_dll_overrides(
    inherited_override: Option<OsString>,
    force_builtin_graphics: bool,
) -> OsString {
    let mut overrides = inherited_override.unwrap_or_else(|| OsString::from("winhttp=n,b"));
    if force_builtin_graphics {
        if !overrides.is_empty() && !overrides.as_encoded_bytes().ends_with(b";") {
            overrides.push(";");
        }
        overrides.push(CROSSOVER_BUILTIN_GRAPHICS_OVERRIDE);
    }
    overrides
}

fn add_crossover_runtime_args(
    args: &mut Vec<OsString>,
    inherited_override: Option<OsString>,
    use_graphics_compatibility: bool,
) {
    let delimiter = args
        .iter()
        .position(|argument| argument == "--")
        .unwrap_or(args.len());
    let mut runtime_args = vec![
        OsString::from("--no-update"),
        OsString::from("--wait-children"),
    ];
    if use_graphics_compatibility {
        runtime_args.extend([
            OsString::from("--env"),
            OsString::from(CROSSOVER_WINED3D_ENV),
        ]);
    }
    runtime_args.extend([
        OsString::from("--dll"),
        crossover_dll_overrides(inherited_override, use_graphics_compatibility),
    ]);
    args.splice(delimiter..delimiter, runtime_args);
}

fn build_program_spec_with_crossover_graphics(
    program: &Path,
    cwd: &Path,
    ctx: &RuntimeContext,
    force_builtin_graphics: bool,
) -> LaunchSpec {
    match ctx.runtime {
        Runtime::Native => LaunchSpec {
            program: program.to_path_buf(),
            args: Vec::new(),
            cwd: cwd.to_path_buf(),
            env: Vec::new(),
            error: None,
        },
        Runtime::Proton => {
            let mut args = ctx.launcher_args.clone();
            args.push(program.as_os_str().to_owned());
            let error = if ctx.launcher.is_none() {
                Some(
                    "Could not find the Proton tool for this Steam installation; select Proton in Steam once, then retry."
                        .to_string(),
                )
            } else if ctx.prefix.is_none() {
                Some("Steam has not created the Among Us Proton prefix yet.".to_string())
            } else if ctx
                .prefix
                .as_deref()
                .and_then(steam_root_from_prefix)
                .is_none()
            {
                Some("Could not locate the Steam client files required by Proton.".to_string())
            } else {
                None
            };
            LaunchSpec {
                program: ctx
                    .launcher
                    .clone()
                    .unwrap_or_else(|| PathBuf::from(runtime_fallback(ctx.runtime))),
                args,
                cwd: cwd.to_path_buf(),
                env: wine_environment(ctx, std::env::var_os("WINEDLLOVERRIDES")),
                error,
            }
        }
        Runtime::Wine | Runtime::Crossover | Runtime::Whisky | Runtime::Bottles => {
            let inherited_override = std::env::var_os("WINEDLLOVERRIDES");
            let env = wine_environment(ctx, inherited_override.clone());
            let mut args = ctx.launcher_args.clone();
            if ctx.runtime == Runtime::Crossover {
                add_crossover_runtime_args(&mut args, inherited_override, force_builtin_graphics);
            }
            args.push(program.as_os_str().to_owned());
            let error = if ctx.runtime == Runtime::Crossover && ctx.launcher.is_none() {
                Some(
                    "Could not find CrossOver's command-line Wine launcher. Install CrossOver in the system or user Applications folder, then retry."
                        .to_string(),
                )
            } else {
                None
            };
            LaunchSpec {
                program: ctx
                    .launcher
                    .clone()
                    .unwrap_or_else(|| PathBuf::from(runtime_fallback(ctx.runtime))),
                args,
                cwd: cwd.to_path_buf(),
                env,
                error,
            }
        }
    }
}

/// Build a concrete launch invocation for an arbitrary Windows executable in a
/// game directory. Epic's authentication helper uses this same runtime path.
pub fn build_program_spec(program: &Path, cwd: &Path, ctx: &RuntimeContext) -> LaunchSpec {
    build_program_spec_with_crossover_graphics(program, cwd, ctx, false)
}

/// Build the concrete launch invocation for Among Us.
pub fn build_launch_spec(game_dir: &Path, ctx: &RuntimeContext) -> LaunchSpec {
    let use_crossover_graphics_compatibility =
        ctx.host == HostPlatform::Macos && ctx.runtime == Runtime::Crossover;
    let mut spec = build_program_spec_with_crossover_graphics(
        &game_dir.join(GAME_EXE),
        game_dir,
        ctx,
        use_crossover_graphics_compatibility,
    );
    if use_crossover_graphics_compatibility {
        spec.args.push("-force-d3d11-bitblt-model".into());
    }
    spec
}

const OVERRIDE_LINE: &str = "\"winhttp\"=\"native,builtin\"";
const OVERRIDE_SECTION: &str = r"[Software\\Wine\\DllOverrides]";
static REGISTRY_TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

fn line_text(line: &str) -> &str {
    let line = line.strip_suffix('\n').unwrap_or(line);
    line.strip_suffix('\r').unwrap_or(line)
}
fn is_override_section_header(line: &str) -> bool {
    let line = line_text(line);
    line.strip_prefix(OVERRIDE_SECTION)
        .map(|rest| rest.is_empty() || rest.chars().next().is_some_and(char::is_whitespace))
        .unwrap_or(false)
}

fn override_section_range(registry: &str) -> Option<(usize, usize)> {
    let mut offset = 0;
    let mut body_start = None;
    for line in registry.split_inclusive('\n') {
        if let Some(start) = body_start {
            if line_text(line).starts_with('[') {
                return Some((start, offset));
            }
        } else if is_override_section_header(line) {
            body_start = Some(offset + line.len());
        }
        offset += line.len();
    }
    body_start.map(|start| (start, registry.len()))
}

fn is_winhttp_value(line: &str) -> bool {
    line.trim_start()
        .split_once('=')
        .map(|(name, _)| name.eq_ignore_ascii_case("\"winhttp\""))
        .unwrap_or(false)
}

fn winhttp_line_range(registry: &str) -> Option<(usize, usize)> {
    let (body_start, body_end) = override_section_range(registry)?;
    let mut offset = body_start;
    for line in registry[body_start..body_end].split_inclusive('\n') {
        let content = line_text(line);
        if is_winhttp_value(content) {
            return Some((offset, offset + content.len()));
        }
        offset += line.len();
    }
    None
}

fn registry_has_winhttp_override(registry: &str) -> bool {
    winhttp_line_range(registry)
        .map(|(start, end)| registry[start..end].trim() == OVERRIDE_LINE)
        .unwrap_or(false)
}

/// Ensure only Wine's DLL override section sets `winhttp` native-first. Values
/// with the same name in unrelated registry sections are left untouched.
pub fn merge_winhttp_override(existing: &str) -> Option<String> {
    if registry_has_winhttp_override(existing) {
        return None;
    }
    if let Some((start, end)) = winhttp_line_range(existing) {
        let indent_len = existing[start..end].len() - existing[start..end].trim_start().len();
        let mut out = String::with_capacity(existing.len() + OVERRIDE_LINE.len());
        out.push_str(&existing[..start + indent_len]);
        out.push_str(OVERRIDE_LINE);
        out.push_str(&existing[end..]);
        return Some(out);
    }

    let newline = if existing.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    if let Some((body_start, _)) = override_section_range(existing) {
        let mut out = String::with_capacity(existing.len() + OVERRIDE_LINE.len() + newline.len());
        out.push_str(&existing[..body_start]);
        if body_start > 0 && !existing[..body_start].ends_with('\n') {
            out.push_str(newline);
        }
        out.push_str(OVERRIDE_LINE);
        out.push_str(newline);
        out.push_str(&existing[body_start..]);
        return Some(out);
    }

    let mut out = String::new();
    if existing.trim().is_empty() {
        out.push_str("WINE REGISTRY Version 2");
        out.push_str(newline);
    } else {
        out.push_str(existing);
        if !out.ends_with('\n') {
            out.push_str(newline);
        }
    }
    out.push_str(newline);
    out.push_str(OVERRIDE_SECTION);
    out.push_str(newline);
    out.push_str(OVERRIDE_LINE);
    out.push_str(newline);
    Some(out)
}

pub fn has_winhttp_override(prefix: &Path) -> bool {
    fs::read_to_string(prefix.join("user.reg"))
        .map(|registry| registry_has_winhttp_override(&registry))
        .unwrap_or(false)
}

fn atomic_replace_with<F>(path: &Path, contents: &[u8], replace: F) -> io::Result<()>
where
    F: FnOnce(&Path, &Path) -> io::Result<()>,
{
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("registry path has no parent directory"))?;
    let base = path.file_name().unwrap_or_default().to_string_lossy();
    let mut temporary = None;
    for _ in 0..32 {
        let sequence = REGISTRY_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let candidate = parent.join(format!(
            ".{base}.perfect-sync-{}-{sequence}.tmp",
            std::process::id()
        ));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => {
                temporary = Some((candidate, file));
                break;
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    let (temporary_path, mut temporary_file) = temporary.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not reserve registry temp file",
        )
    })?;

    let result = (|| {
        if let Ok(metadata) = fs::metadata(path) {
            temporary_file.set_permissions(metadata.permissions())?;
        }
        temporary_file.write_all(contents)?;
        temporary_file.sync_all()?;
        drop(temporary_file);
        replace(&temporary_path, path)?;
        #[cfg(unix)]
        let _ = fs::File::open(parent).and_then(|directory| directory.sync_all());
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary_path);
    }
    result
}

#[cfg(windows)]
fn replace_existing_file(replacement: &Path, destination: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use std::ptr;

    #[link(name = "Kernel32")]
    extern "system" {
        fn ReplaceFileW(
            replaced_file_name: *const u16,
            replacement_file_name: *const u16,
            backup_file_name: *const u16,
            replace_flags: u32,
            exclude: *mut std::ffi::c_void,
            reserved: *mut std::ffi::c_void,
        ) -> i32;
    }

    fn wide_path(path: &Path) -> io::Result<Vec<u16>> {
        let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
        if wide.contains(&0) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "registry path contains a NUL character",
            ));
        }
        wide.push(0);
        Ok(wide)
    }

    let parent = destination
        .parent()
        .ok_or_else(|| io::Error::other("registry path has no parent directory"))?;
    let base = destination
        .file_name()
        .unwrap_or_default()
        .to_string_lossy();
    let sequence = REGISTRY_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let backup = parent.join(format!(
        ".{base}.perfect-sync-{}-{sequence}.backup",
        std::process::id()
    ));

    let destination_wide = wide_path(destination)?;
    let replacement_wide = wide_path(replacement)?;
    let backup_wide = wide_path(&backup)?;
    let replaced = unsafe {
        ReplaceFileW(
            destination_wide.as_ptr(),
            replacement_wide.as_ptr(),
            backup_wide.as_ptr(),
            0,
            ptr::null_mut(),
            ptr::null_mut(),
        )
    };
    if replaced != 0 {
        let _ = fs::remove_file(backup);
        return Ok(());
    }

    let error = io::Error::last_os_error();
    // ERROR_UNABLE_TO_MOVE_REPLACEMENT_2 leaves the old destination at the
    // requested backup path. Restore it without ever deleting the destination.
    if error.raw_os_error() == Some(1177) {
        let _ = fs::rename(&backup, destination);
    }
    Err(error)
}

#[cfg(not(windows))]
fn replace_existing_file(replacement: &Path, destination: &Path) -> io::Result<()> {
    fs::rename(replacement, destination)
}

fn atomic_replace(path: &Path, contents: &[u8]) -> io::Result<()> {
    atomic_replace_with(path, contents, replace_existing_file)
}

/// Write and verify the winhttp override in a prefix's `user.reg`.
/// A missing prefix is an actionable error: Proton creates it on the game's
/// first vanilla launch, and silently skipping here would produce a broken modded launch.
pub fn register_winhttp_override(prefix: &Path) -> io::Result<()> {
    if !prefix.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("Wine/Proton prefix does not exist: {}", prefix.display()),
        ));
    }
    let reg = prefix.join("user.reg");
    let existing = fs::read_to_string(&reg).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("could not read Wine registry {}: {error}", reg.display()),
        )
    })?;
    if let Some(updated) = merge_winhttp_override(&existing) {
        atomic_replace(&reg, updated.as_bytes())?;
    }
    let verified = fs::read_to_string(&reg)?;
    if !registry_has_winhttp_override(&verified) {
        return Err(io::Error::other(
            "winhttp override did not persist in user.reg",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context(runtime: Runtime, launcher: &str) -> RuntimeContext {
        RuntimeContext {
            host: HostPlatform::Linux,
            runtime,
            prefix: None,
            launcher: Some(PathBuf::from(launcher)),
            launcher_args: Vec::new(),
        }
    }

    #[test]
    fn proton_prefix_derives_compatdata() {
        let game = Path::new("/home/u/.steam/steam/steamapps/common/Among Us");
        assert_eq!(
            proton_prefix_from_game(game),
            Some(PathBuf::from(
                "/home/u/.steam/steam/steamapps/compatdata/945360/pfx"
            ))
        );
    }

    #[test]
    fn wine_prefix_is_drive_c_parent() {
        let game = Path::new("/home/u/.wine/drive_c/Program Files/Among Us");
        assert_eq!(
            wine_prefix_from_game(game),
            Some(PathBuf::from("/home/u/.wine"))
        );
        assert_eq!(wine_prefix_from_game(Path::new("/no/prefix/here")), None);
    }

    #[test]
    fn bottle_detection_precedes_steamapps() {
        let mac = Path::new(
            "/Users/u/Library/Application Support/CrossOver/Bottles/AU/drive_c/Program Files (x86)/Steam/steamapps/common/Among Us",
        );
        assert_eq!(
            classify_runtime(mac, HostPlatform::Macos, None).0,
            Runtime::Crossover
        );
        let linux =
            Path::new("/home/u/.wine/drive_c/Program Files (x86)/Steam/steamapps/common/Among Us");
        assert_eq!(
            classify_runtime(linux, HostPlatform::Linux, None).0,
            Runtime::Wine
        );
    }

    #[test]
    fn proton_is_linux_only() {
        let game = Path::new("/games/steamapps/common/Among Us");
        assert_eq!(
            classify_runtime(game, HostPlatform::Linux, None).0,
            Runtime::Proton
        );
        assert_eq!(
            classify_runtime(game, HostPlatform::Macos, None).0,
            Runtime::Wine
        );
    }

    #[test]
    fn persisted_bottle_runtime_handles_custom_locations() {
        let game = Path::new("/custom/AU/drive_c/Games/Among Us");
        assert_eq!(
            classify_runtime(game, HostPlatform::Macos, Some(Runtime::Whisky)).0,
            Runtime::Whisky
        );
    }

    #[test]
    fn native_spec_runs_exe_directly() {
        let game = Path::new("/g/Among Us");
        let mut ctx = context(Runtime::Native, "unused");
        ctx.host = HostPlatform::Windows;
        ctx.launcher = None;
        let spec = build_launch_spec(game, &ctx);
        assert!(spec.program.ends_with("Among Us.exe"));
        assert!(spec.args.is_empty());
        assert!(spec.env.is_empty());
    }

    #[test]
    fn proton_spec_runs_the_managed_executable_directly() {
        let game = Path::new("/managed/workspace/current");
        let mut ctx = context(Runtime::Proton, "/g/steamapps/common/Proton 10.0/proton");
        ctx.prefix = Some(PathBuf::from("/g/steamapps/compatdata/945360/pfx"));
        ctx.launcher_args = vec!["run".into()];
        let spec = build_launch_spec(game, &ctx);
        assert_eq!(
            spec.program,
            PathBuf::from("/g/steamapps/common/Proton 10.0/proton")
        );
        assert_eq!(spec.args[0], "run");
        assert!(Path::new(&spec.args[1]).ends_with("Among Us.exe"));
        assert!(spec.args[1]
            .to_string_lossy()
            .contains("managed/workspace/current"));
        assert!(!spec
            .args
            .iter()
            .any(|arg| arg == "-applaunch" || arg == STEAM_APP_ID));
        assert!(spec.env.iter().any(|(key, value)| {
            key == "STEAM_COMPAT_DATA_PATH" && value == Path::new("/g/steamapps/compatdata/945360")
        }));
        assert!(spec.env.iter().any(|(key, value)| {
            key == "STEAM_COMPAT_CLIENT_INSTALL_PATH" && value == Path::new("/g")
        }));
        assert!(spec
            .env
            .iter()
            .any(|(key, value)| key == "SteamAppId" && value == STEAM_APP_ID));
    }

    #[test]
    fn missing_proton_tool_fails_closed_without_an_app_id_launch() {
        let game = Path::new("/managed/workspace/current");
        let mut ctx = context(Runtime::Proton, "proton");
        ctx.prefix = Some(PathBuf::from("/g/steamapps/compatdata/945360/pfx"));
        ctx.launcher = None;
        ctx.launcher_args = vec!["run".into()];
        let spec = build_launch_spec(game, &ctx);
        assert!(spec.error.is_some());
        assert!(!spec
            .args
            .iter()
            .any(|arg| arg == "-applaunch" || arg == STEAM_APP_ID));
        assert_eq!(
            crate::process::launch(&spec).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
    }

    #[test]
    fn wine_spec_sets_overrides_and_prefix() {
        let game = Path::new("/b/drive_c/Among Us");
        let mut ctx = context(Runtime::Wine, "/usr/bin/wine");
        ctx.prefix = Some(PathBuf::from("/b"));
        let spec = build_launch_spec(game, &ctx);
        assert_eq!(spec.program, PathBuf::from("/usr/bin/wine"));
        assert!(Path::new(&spec.args[0]).ends_with("Among Us.exe"));
        let env = wine_environment(&ctx, None);
        assert!(env
            .iter()
            .any(|(k, v)| k == "WINEDLLOVERRIDES" && v == "winhttp=n,b"));
        assert!(env.iter().any(|(k, v)| k == "WINEPREFIX" && v == "/b"));
    }

    #[test]
    fn inherited_dll_overrides_and_bottle_environment_are_preserved() {
        let mut ctx = context(
            Runtime::Crossover,
            "/Applications/CrossOver.app/Contents/SharedSupport/CrossOver/bin/wine",
        );
        ctx.prefix = Some(PathBuf::from("/CrossOver/Bottles/Existing Bottle"));
        let env = wine_environment(&ctx, Some(OsString::from("custom=n")));
        assert!(!env.iter().any(|(key, _)| key == "WINEDLLOVERRIDES"));
        assert!(env
            .iter()
            .any(|(key, value)| key == "CX_BOTTLE" && value == "Existing Bottle"));
    }

    #[test]
    fn crossover_spec_uses_wine_and_selects_bottle() {
        let game = Path::new("/CrossOver/Bottles/AU/drive_c/Games/Among Us");
        let mut ctx = context(
            Runtime::Crossover,
            "/Applications/CrossOver.app/Contents/SharedSupport/CrossOver/bin/wine",
        );
        ctx.prefix = Some(PathBuf::from("/CrossOver/Bottles/AU"));
        ctx.launcher_args = vec!["--bottle".into(), "AU".into(), "--".into()];
        ctx.host = HostPlatform::Macos;
        let spec = build_launch_spec(game, &ctx);
        assert_eq!(
            &spec.args[..7],
            [
                "--bottle",
                "AU",
                "--no-update",
                "--wait-children",
                "--env",
                CROSSOVER_WINED3D_ENV,
                "--dll",
            ]
        );
        assert_eq!(
            spec.args[7],
            crossover_dll_overrides(std::env::var_os("WINEDLLOVERRIDES"), true)
        );
        assert_eq!(spec.args[8], "--");
        assert_eq!(Path::new(&spec.args[9]), game.join(GAME_EXE));
        assert_eq!(spec.args[10], "-force-d3d11-bitblt-model");
        assert!(spec.program.ends_with("bin/wine"));
        assert!(spec.env.iter().any(|(k, v)| k == "CX_BOTTLE" && v == "AU"));
    }

    #[test]
    fn crossover_bitblt_override_is_limited_to_macos_game_launches() {
        let game = Path::new("/CrossOver/Bottles/AU/drive_c/Games/Among Us");
        let mut ctx = context(
            Runtime::Crossover,
            "/Applications/CrossOver.app/Contents/SharedSupport/CrossOver/bin/wine",
        );
        ctx.prefix = Some(PathBuf::from("/CrossOver/Bottles/AU"));
        ctx.launcher_args = vec!["--bottle".into(), "AU".into(), "--".into()];

        let game_spec = build_launch_spec(game, &ctx);
        assert!(!game_spec
            .args
            .iter()
            .any(|argument| argument == "-force-d3d11-bitblt-model"));
        assert!(!game_spec
            .args
            .iter()
            .any(|argument| argument == CROSSOVER_WINED3D_ENV));
        assert_eq!(
            game_spec.args[5],
            crossover_dll_overrides(std::env::var_os("WINEDLLOVERRIDES"), false)
        );

        ctx.host = HostPlatform::Macos;
        let helper_spec = build_program_spec(&game.join("helper.exe"), game, &ctx);
        assert!(!helper_spec
            .args
            .iter()
            .any(|argument| argument == "-force-d3d11-bitblt-model"));
        assert!(!helper_spec
            .args
            .iter()
            .any(|argument| argument == CROSSOVER_WINED3D_ENV));
        assert_eq!(
            helper_spec.args[5],
            crossover_dll_overrides(std::env::var_os("WINEDLLOVERRIDES"), false)
        );
    }

    #[test]
    fn crossover_runtime_args_preserve_inherited_dll_overrides() {
        let mut args = vec!["--bottle".into(), "AU".into(), "--".into()];
        add_crossover_runtime_args(&mut args, Some(OsString::from("custom=n")), false);
        assert_eq!(
            args,
            [
                "--bottle",
                "AU",
                "--no-update",
                "--wait-children",
                "--dll",
                "custom=n",
                "--",
            ]
        );
    }

    #[test]
    fn crossover_runtime_args_force_wined3d_after_inherited_overrides() {
        let mut args = vec!["--bottle".into(), "AU".into(), "--".into()];
        add_crossover_runtime_args(&mut args, Some(OsString::from("custom=n;d3d11=n")), true);
        assert_eq!(
            args,
            [
                "--bottle",
                "AU",
                "--no-update",
                "--wait-children",
                "--env",
                CROSSOVER_WINED3D_ENV,
                "--dll",
                "custom=n;d3d11=n;d3d11,dxgi=b",
                "--",
            ]
        );
        assert_eq!(
            crossover_dll_overrides(None, true),
            "winhttp=n,b;d3d11,dxgi=b"
        );
    }

    #[test]
    fn crossover_wine_is_found_in_current_and_versioned_app_bundles() {
        let temp = tempfile::tempdir().unwrap();
        let current = temp
            .path()
            .join("CrossOver.app")
            .join(CROSSOVER_WINE_RELATIVE_PATH);
        fs::create_dir_all(current.parent().unwrap()).unwrap();
        fs::write(&current, b"wine").unwrap();
        assert_eq!(
            find_crossover_wine_in_applications(temp.path()),
            Some(current.clone())
        );

        fs::remove_dir_all(temp.path().join("CrossOver.app")).unwrap();
        let versioned = temp
            .path()
            .join("CrossOver-Preview.app")
            .join(CROSSOVER_WINE_RELATIVE_PATH);
        fs::create_dir_all(versioned.parent().unwrap()).unwrap();
        fs::write(&versioned, b"wine").unwrap();
        assert_eq!(
            find_crossover_wine_in_applications(temp.path()),
            Some(versioned)
        );
    }

    #[test]
    fn missing_crossover_wine_fails_before_spawning_a_bare_command() {
        let game = Path::new("/managed/workspace/current");
        let mut ctx = context(Runtime::Crossover, "wine");
        ctx.launcher = None;
        ctx.prefix = Some(PathBuf::from("/CrossOver/Bottles/AU"));
        ctx.launcher_args = vec!["--bottle".into(), "AU".into(), "--".into()];
        let spec = build_launch_spec(game, &ctx);
        assert!(spec
            .error
            .as_deref()
            .is_some_and(|message| { message.contains("CrossOver's command-line Wine launcher") }));
        assert_eq!(
            crate::process::launch(&spec).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
    }

    #[test]
    fn whisky_spec_uses_start_unix_in_the_selected_prefix() {
        let game = Path::new("/Whisky/Bottles/AU/drive_c/Games/Among Us");
        let mut ctx = context(Runtime::Whisky, "/Whisky/Libraries/Wine/bin/wine64");
        ctx.prefix = Some(PathBuf::from("/Whisky/Bottles/AU"));
        ctx.launcher_args = vec!["start".into(), "/unix".into()];
        let spec = build_launch_spec(game, &ctx);
        assert!(Path::new(&spec.args[2]).ends_with("Among Us.exe"));
        assert!(spec
            .env
            .iter()
            .any(|(key, value)| key == "WINEPREFIX" && value == "/Whisky/Bottles/AU"));
    }

    #[test]
    fn bottles_spec_selects_the_detected_bottle() {
        let game = Path::new("/bottles/bottles/AU/drive_c/Games/Among Us");
        let mut ctx = context(Runtime::Bottles, "/usr/bin/bottles-cli");
        ctx.prefix = Some(PathBuf::from("/bottles/bottles/AU"));
        ctx.launcher_args = vec!["run".into(), "-b".into(), "AU".into(), "-e".into()];
        let spec = build_launch_spec(game, &ctx);
        assert!(Path::new(&spec.args[4]).ends_with("Among Us.exe"));
        assert!(!spec.env.iter().any(|(key, _)| key == "WINEPREFIX"));
    }

    #[test]
    fn override_added_to_empty_reg() {
        let out = merge_winhttp_override("").unwrap();
        assert!(out.contains(OVERRIDE_SECTION));
        assert!(out.contains(OVERRIDE_LINE));
        assert!(out.starts_with("WINE REGISTRY Version 2"));
    }

    #[test]
    fn override_inserted_into_existing_section() {
        let reg = "WINE REGISTRY Version 2\n\n[Software\\\\Wine\\\\DllOverrides] 1700000000\n\"other\"=\"builtin\"\n";
        let out = merge_winhttp_override(reg).unwrap();
        assert!(out.contains(OVERRIDE_LINE));
        assert!(out.contains("\"other\"=\"builtin\""));
    }

    #[test]
    fn override_idempotent_when_present() {
        let reg = "[Software\\\\Wine\\\\DllOverrides] 1\n\"winhttp\"=\"native,builtin\"\n";
        assert_eq!(merge_winhttp_override(reg), None);
    }

    #[test]
    fn override_replaces_stale_winhttp_value() {
        let reg = "[Software\\\\Wine\\\\DllOverrides] 1\n\"winhttp\"=\"builtin\"\n";
        let out = merge_winhttp_override(reg).unwrap();
        assert!(out.contains(OVERRIDE_LINE));
        assert!(!out.contains("\"winhttp\"=\"builtin\""));
    }

    #[test]
    fn unrelated_winhttp_values_are_never_detected_or_edited() {
        let reg = concat!(
            "WINE REGISTRY Version 2\n\n",
            "[Software\\\\Vendor\\\\Settings] 1\n",
            "\"winhttp\"=\"native,builtin\"\n",
            "[Software\\\\Wine\\\\DllOverrides] 2\n",
            "\"other\"=\"builtin\"\n",
            "[Software\\\\Other] 3\n",
            "\"winhttp\"=\"builtin\"\n",
        );
        assert!(!registry_has_winhttp_override(reg));
        let out = merge_winhttp_override(reg).unwrap();
        assert!(registry_has_winhttp_override(&out));
        assert!(out.contains("[Software\\\\Vendor\\\\Settings] 1\n\"winhttp\"=\"native,builtin\""));
        assert!(out.contains("[Software\\\\Other] 3\n\"winhttp\"=\"builtin\""));
    }

    #[test]
    fn atomic_replace_overwrites_existing_registry_on_this_platform() {
        let tmp = tempfile::tempdir().unwrap();
        let reg = tmp.path().join("user.reg");
        let original = concat!(
            "WINE REGISTRY Version 2\n\n",
            "[Software\\\\Vendor\\\\Settings] 1\n",
            "\"preserve\"=\"these bytes\"\n",
            "[Software\\\\Wine\\\\DllOverrides] 2\n",
            "\"winhttp\"=\"builtin\"\n",
        );
        fs::write(&reg, original).unwrap();
        let replacement = merge_winhttp_override(original).unwrap();

        atomic_replace(&reg, replacement.as_bytes()).unwrap();

        assert_eq!(fs::read(&reg).unwrap(), replacement.as_bytes());
        assert!(replacement
            .contains("[Software\\\\Vendor\\\\Settings] 1\n\"preserve\"=\"these bytes\"\n"));
        assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 1);
    }

    #[test]
    fn failed_atomic_replace_keeps_original_registry() {
        let tmp = tempfile::tempdir().unwrap();
        let reg = tmp.path().join("user.reg");
        let original = concat!(
            "WINE REGISTRY Version 2\n\n",
            "[Software\\\\Vendor\\\\Settings] 1\n",
            "\"preserve\"=\"byte-identical on failure\"\n",
        )
        .as_bytes();
        fs::write(&reg, original).unwrap();
        let error = atomic_replace_with(&reg, b"replacement", |_, _| {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "injected failure",
            ))
        })
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(fs::read(&reg).unwrap(), original);
        assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 1);
    }

    #[test]
    fn override_requires_an_existing_prefix() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("missing");
        assert_eq!(
            register_winhttp_override(&missing).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
    }

    #[test]
    fn override_write_is_verified() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("user.reg"), "WINE REGISTRY Version 2\n").unwrap();
        register_winhttp_override(tmp.path()).unwrap();
        assert!(has_winhttp_override(tmp.path()));
    }

    #[test]
    fn current_host_classification_smoke() {
        let game = Path::new("/games/steamapps/common/Among Us");
        let runtime = classify_runtime(game, current_host(), None).0;
        if cfg!(target_os = "windows") {
            assert_eq!(runtime, Runtime::Native);
        } else if cfg!(target_os = "linux") {
            assert_eq!(runtime, Runtime::Proton);
        } else {
            assert_eq!(runtime, Runtime::Wine);
        }
    }
}
