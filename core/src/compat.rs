//! Cross-platform launch + loader compatibility.
//!
//! Among Us is a Windows-only Unity build, so the BepInEx Doorstop files we
//! install are always the Windows pack. What changes per host is *how the game
//! runs*: on Linux it is Steam Proton, on macOS CrossOver or Wine/Whisky. This
//! module resolves the runtime for a game dir, registers the `winhttp` DLL
//! override the Doorstop needs inside the Wine prefix (so BepInEx loads no
//! matter how the user starts the game), and builds the launch invocation.

use crate::process::{LaunchSpec, GAME_EXE};
use crate::types::Runtime;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Among Us Steam app id (its Proton prefix lives at `compatdata/<id>/pfx`).
pub const STEAM_APP_ID: &str = "945360";

/// How (and where) a given game dir should be launched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeContext {
    pub runtime: Runtime,
    /// Wine prefix (the dir holding `user.reg`): Proton's `compatdata/<id>/pfx`
    /// or a Wine/CrossOver bottle. `None` on native Windows.
    pub prefix: Option<PathBuf>,
    /// Binary used to start the game (steam / wine / CrossOver's wine).
    /// `None` means run the exe directly (native).
    pub launcher: Option<PathBuf>,
}

/// Resolve how a game dir should run on this host. Pure dispatch by `cfg!` plus
/// path probing, so a manual game-path override still launches correctly.
pub fn resolve(game_dir: &Path) -> RuntimeContext {
    if cfg!(windows) {
        return RuntimeContext { runtime: Runtime::Native, prefix: None, launcher: None };
    }
    let p = game_dir.to_string_lossy().replace('\\', "/");
    // Steam library layout -> Proton (Steam manages the prefix + Proton version).
    if p.contains("/steamapps/") {
        return RuntimeContext {
            runtime: Runtime::Proton,
            prefix: proton_prefix_from_game(game_dir),
            launcher: find_binary(&["steam"]),
        };
    }
    let prefix = wine_prefix_from_game(game_dir);
    if cfg!(target_os = "macos") && p.contains("/CrossOver/") {
        return RuntimeContext {
            runtime: Runtime::Crossover,
            prefix,
            launcher: find_crossover_wine().or_else(|| find_binary(&["wine"])),
        };
    }
    RuntimeContext { runtime: Runtime::Wine, prefix, launcher: find_binary(&["wine64", "wine"]) }
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

/// Wine prefix (the dir containing `drive_c`) for a game dir inside a bottle.
pub fn wine_prefix_from_game(game_dir: &Path) -> Option<PathBuf> {
    for anc in game_dir.ancestors() {
        if anc.file_name().and_then(|n| n.to_str()) == Some("drive_c") {
            return anc.parent().map(Path::to_path_buf);
        }
    }
    None
}

/// First existing `name` across the `PATH` entries.
fn find_binary(names: &[&str]) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        for name in names {
            let cand = dir.join(name);
            if cand.is_file() {
                return Some(cand);
            }
        }
    }
    None
}

fn find_crossover_wine() -> Option<PathBuf> {
    let p =
        PathBuf::from("/Applications/CrossOver.app/Contents/SharedSupport/CrossOver/bin/wine");
    p.is_file().then_some(p)
}

/// Build the concrete launch invocation for a resolved runtime. Pure: every
/// branch is selected by `ctx.runtime` (a value), so all paths are testable on
/// any host.
pub fn build_launch_spec(game_dir: &Path, ctx: &RuntimeContext) -> LaunchSpec {
    let exe = game_dir.join(GAME_EXE);
    match ctx.runtime {
        Runtime::Native => LaunchSpec {
            program: exe,
            args: Vec::new(),
            cwd: game_dir.to_path_buf(),
            env: Vec::new(),
        },
        // Let Steam pick the user's Proton + launch options; the winhttp override
        // we wrote into the prefix is what makes the Doorstop load.
        Runtime::Proton => LaunchSpec {
            program: ctx.launcher.clone().unwrap_or_else(|| PathBuf::from("steam")),
            args: vec!["-applaunch".to_string(), STEAM_APP_ID.to_string()],
            cwd: game_dir.to_path_buf(),
            env: Vec::new(),
        },
        Runtime::Wine | Runtime::Crossover => {
            let mut env = vec![("WINEDLLOVERRIDES".to_string(), "winhttp=n,b".to_string())];
            if let Some(prefix) = &ctx.prefix {
                env.push((
                    "WINEPREFIX".to_string(),
                    prefix.to_string_lossy().into_owned(),
                ));
            }
            let fallback = if ctx.runtime == Runtime::Crossover { "wine" } else { "wine64" };
            LaunchSpec {
                program: ctx.launcher.clone().unwrap_or_else(|| PathBuf::from(fallback)),
                args: vec![exe.to_string_lossy().into_owned()],
                cwd: game_dir.to_path_buf(),
                env,
            }
        }
    }
}

const OVERRIDE_LINE: &str = "\"winhttp\"=\"native,builtin\"";
const OVERRIDE_SECTION: &str = r"[Software\\Wine\\DllOverrides]";

/// Ensure a Wine `user.reg` sets `winhttp` to load native-first (so the BepInEx
/// Doorstop proxy is used instead of Wine's builtin). Returns the updated text,
/// or `None` when the override is already correct (no write needed).
pub fn merge_winhttp_override(existing: &str) -> Option<String> {
    if existing.contains(OVERRIDE_LINE) {
        return None;
    }
    // A different winhttp override is set -> replace that whole line.
    if let Some(start) = existing.find("\"winhttp\"=") {
        let end = existing[start..].find('\n').map(|n| start + n).unwrap_or(existing.len());
        let mut out = String::with_capacity(existing.len());
        out.push_str(&existing[..start]);
        out.push_str(OVERRIDE_LINE);
        out.push_str(&existing[end..]);
        return Some(out);
    }
    // Section exists -> insert the value right after its header line.
    if let Some(idx) = existing.find(OVERRIDE_SECTION) {
        let after_header =
            existing[idx..].find('\n').map(|n| idx + n + 1).unwrap_or(existing.len());
        let mut out = String::with_capacity(existing.len() + OVERRIDE_LINE.len() + 1);
        out.push_str(&existing[..after_header]);
        out.push_str(OVERRIDE_LINE);
        out.push('\n');
        out.push_str(&existing[after_header..]);
        return Some(out);
    }
    // No section -> append one (with a registry header if the file was empty).
    let mut out = String::new();
    if existing.trim().is_empty() {
        out.push_str("WINE REGISTRY Version 2\n");
    } else {
        out.push_str(existing);
        if !out.ends_with('\n') {
            out.push('\n');
        }
    }
    out.push('\n');
    out.push_str(OVERRIDE_SECTION);
    out.push('\n');
    out.push_str(OVERRIDE_LINE);
    out.push('\n');
    Some(out)
}

/// Write the winhttp override into a prefix's `user.reg` (idempotent,
/// best-effort). No-op when the prefix dir does not exist yet.
pub fn register_winhttp_override(prefix: &Path) -> io::Result<()> {
    if !prefix.is_dir() {
        return Ok(());
    }
    let reg = prefix.join("user.reg");
    let existing = fs::read_to_string(&reg).unwrap_or_default();
    if let Some(updated) = merge_winhttp_override(&existing) {
        fs::write(&reg, updated)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(wine_prefix_from_game(game), Some(PathBuf::from("/home/u/.wine")));
        assert_eq!(wine_prefix_from_game(Path::new("/no/prefix/here")), None);
    }

    #[test]
    fn native_spec_runs_exe_directly() {
        let game = Path::new("/g/Among Us");
        let ctx = RuntimeContext { runtime: Runtime::Native, prefix: None, launcher: None };
        let spec = build_launch_spec(game, &ctx);
        assert!(spec.program.ends_with("Among Us.exe"));
        assert!(spec.args.is_empty());
        assert!(spec.env.is_empty());
    }

    #[test]
    fn proton_spec_launches_via_steam() {
        let game = Path::new("/g/steamapps/common/Among Us");
        let ctx = RuntimeContext {
            runtime: Runtime::Proton,
            prefix: Some(PathBuf::from("/g/steamapps/compatdata/945360/pfx")),
            launcher: Some(PathBuf::from("/usr/bin/steam")),
        };
        let spec = build_launch_spec(game, &ctx);
        assert_eq!(spec.program, PathBuf::from("/usr/bin/steam"));
        assert_eq!(spec.args, vec!["-applaunch".to_string(), "945360".to_string()]);
    }

    #[test]
    fn wine_spec_sets_overrides_and_prefix() {
        let game = Path::new("/b/drive_c/Among Us");
        let ctx = RuntimeContext {
            runtime: Runtime::Wine,
            prefix: Some(PathBuf::from("/b")),
            launcher: Some(PathBuf::from("/usr/bin/wine")),
        };
        let spec = build_launch_spec(game, &ctx);
        assert_eq!(spec.program, PathBuf::from("/usr/bin/wine"));
        assert!(spec.args[0].ends_with("Among Us.exe"));
        assert!(spec
            .env
            .iter()
            .any(|(k, v)| k == "WINEDLLOVERRIDES" && v == "winhttp=n,b"));
        assert!(spec
            .env
            .iter()
            .any(|(k, v)| k == "WINEPREFIX" && v == "/b"));
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
        // existing value preserved
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
}
