//! Tauri commands: thin adapters over `perfect-sync-core`. Heavy logic lives in
//! the (tested) core crate; these wrap it for the frontend and map errors to
//! strings. The backend is authoritative for profile persistence on disk.
//!
//! Network/disk-heavy commands are `async` and run their blocking body on a
//! worker thread via `spawn_blocking`, so the UI thread never freezes.

use crate::managed_instance;
use crate::settings::{self, Settings, SettingsView, TokenAction};
use crate::storage;
use atomicwrites::{AllowOverwrite, AtomicFile};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use perfect_sync_core::catalog::{parse, AssetArchRule, AssetRules, Catalog};
use perfect_sync_core::deps;
use perfect_sync_core::preview::{preview, Preview};
use perfect_sync_core::profile::{InstalledMod, ProfileRecord, ProfileStore};
use perfect_sync_core::resolver::{
    download_resolved as download_resolved_uncached, download_resolved_to_writer, Http, Release,
    ResolvedDownload, UreqHttp,
};
use perfect_sync_core::types::{
    valid_levelimposter_map_id, Arch, ModSource, ModTag, Runtime, Store, Trust,
};
use perfect_sync_core::{codec, compat, game, loader, process, profile, resolver};
use secrecy::ExposeSecret;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Cursor, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{ipc::Channel, AppHandle, Manager};

const BUNDLED_CATALOG: &str = include_str!("../../catalog/catalog.json");

const DEFAULT_CATALOG_URL: &str =
    "https://raw.githubusercontent.com/artriy/Perfect-Sync/main/catalog/catalog.json";
const MAX_CATALOG_BYTES: u64 = 8 * 1024 * 1024;
const MAX_USER_CATALOG_BYTES: u64 = 2 * 1024 * 1024;
const MAX_CATALOG_ENVELOPE_BYTES: u64 = MAX_CATALOG_BYTES * 2 + MAX_USER_CATALOG_BYTES + 64 * 1024;
const DOORSTOP_FIX_VERSION: &str = "4.5.1";
const DOORSTOP_FIX_URL: &str =
    "https://github.com/Pietrodjaowjao/UnityDoorstop/releases/download/v4.5.1/doorstop_win_release.zip";
const DOORSTOP_FIX_SIZE: usize = 34_391;
const DOORSTOP_FIX_SHA256: &str =
    "c729811c724395d871e97ff2f49be71963951147ed8b878cb0be5d2e439b55b7";
const PINNED_LOADER_VERSION: &str = "be.753+0d275a4";
const PINNED_LOADER_X86_URL: &str =
    "https://builds.bepinex.dev/projects/bepinex_be/753/BepInEx-Unity.IL2CPP-win-x86-6.0.0-be.753%2B0d275a4.zip";
const PINNED_LOADER_X64_URL: &str =
    "https://builds.bepinex.dev/projects/bepinex_be/753/BepInEx-Unity.IL2CPP-win-x64-6.0.0-be.753%2B0d275a4.zip";
const TOU_MIRA_ID: &str = "AU-Avengers/TOU-Mira";
const TOU_BUNDLED_DEPENDENCY_IDS: &[&str] = &[
    "All-Of-Us-Mods/MiraAPI",
    "NuclearPowered/Reactor",
    "miniduikboot/Mini.RegionInstall",
];
const TOU_BUNDLED_PLUGIN_FILES: &[&str] = &["Mini.RegionInstall.dll", "MiraAPI.dll", "Reactor.dll"];
const PRIORITY_CATALOG_IDS: &[&str] = &[
    "TheOtherRolesAU/TheOtherRoles",
    TOU_MIRA_ID,
    "EnhancedNetwork/TownofHost-Enhanced",
    "Mehzxzz/TownOfExtra",
    LEVELIMPOSTER_ID,
];
const MAX_TOU_PACKAGE_CACHE_BYTES: u64 = 512 * 1024 * 1024;
const LEVELIMPOSTER_API: &str = "https://api.levelimposter.net";
const LEVELIMPOSTER_ALGOLIA_URL: &str =
    "https://T5IVXJGKB9-dsn.algolia.net/1/indexes/LevelImposter-Maps";
const LEVELIMPOSTER_ALGOLIA_APP_ID: &str = "T5IVXJGKB9";
const LEVELIMPOSTER_ALGOLIA_SEARCH_KEY: &str = "14062d24b40e0b3689a899fc36abd756";
const LEVELIMPOSTER_ID: &str = "DigiWorm0/LevelImposter";
const MAX_LEVELIMPOSTER_MAPS_PER_BATCH: usize = 32;
const MAX_LEVELIMPOSTER_MAP_BYTES: u64 = 256 * 1024 * 1024;
const MAX_LEVELIMPOSTER_MAP_TOTAL_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_LEVELIMPOSTER_BANNER_BYTES: usize = 4 * 1024 * 1024;
const MAX_PROFILE_STAGE_FILES: usize = 8_192;
const MAX_PROFILE_STAGE_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_PROFILE_RECOVERY_JOURNALS: usize = 64;
const MAX_PROFILE_RECOVERY_PARENT_ENTRIES: usize = 4_096;
const MAX_PROFILE_RECOVERY_JOURNAL_BYTES: u64 = 1_024;
const MAX_RECURSIVE_COPY_FILES: usize = 200_000;
const MAX_RECURSIVE_COPY_BYTES: u64 = 32 * 1024 * 1024 * 1024;
const MAX_ERROR_LOG_BYTES: u64 = 256 * 1024 * 1024;
const CROSSOVER_GAME_START_TIMEOUT: Duration = Duration::from_secs(300);
const CROSSOVER_GAME_STABILITY: Duration = Duration::from_secs(3);
const CROSSOVER_STEAM_STABILITY: Duration = Duration::from_secs(10);
const CROSSOVER_STEAM_START_TIMEOUT: Duration = Duration::from_secs(60);
const CROSSOVER_OUTPUT_BYTES_PER_STREAM: usize = 8 * 1024;
const CROSSOVER_OUTPUT_DRAIN_TIMEOUT: Duration = Duration::from_millis(250);
const MAX_SUPPORT_EVENT_BYTES: usize = 64 * 1024;
static MUTATION_LOCK: Mutex<()> = Mutex::new(());
static LAUNCH_PENDING: LazyLock<Mutex<HashSet<String>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));
static SUPPORT_TOKEN_PATTERN: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?i)\b(?:github_pat_[A-Za-z0-9_]{20,}|gh[pousr]_[A-Za-z0-9_]{20,})\b")
        .expect("static token regex")
});
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);
static INSPECTED_GAMES: LazyLock<Mutex<HashSet<PathBuf>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));
static HTTP_CLIENT: LazyLock<Mutex<Option<(String, UreqHttp)>>> =
    LazyLock::new(|| Mutex::new(None));
const ASSET_CACHE_LOCK_SHARDS: usize = 32;
static ASSET_CACHE_LOCKS: LazyLock<[Mutex<()>; ASSET_CACHE_LOCK_SHARDS]> =
    LazyLock::new(|| std::array::from_fn(|_| Mutex::new(())));
const PROFILE_LOCK_SHARDS: usize = 64;
static PROFILE_LOCKS: LazyLock<[Mutex<()>; PROFILE_LOCK_SHARDS]> =
    LazyLock::new(|| std::array::from_fn(|_| Mutex::new(())));
static PROFILE_RECOVERY_LOCK: Mutex<()> = Mutex::new(());

/// Run a blocking closure off the UI thread and flatten the result.
async fn blocking<T, F>(f: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| e.to_string())?
}

fn log_perf(label: &str, started: Instant, files: usize, bytes: u64) {
    log::info!(
        target: "perfect_sync::performance",
        "{label} completed in {} ms ({files} files, {bytes} bytes)",
        started.elapsed().as_millis()
    );
}

fn lock_mutations() -> Result<std::sync::MutexGuard<'static, ()>, String> {
    MUTATION_LOCK
        .lock()
        .map_err(|_| "backend mutation lock is poisoned".to_string())
}

fn lock_profile_mutation(profile_id: &str) -> Result<std::sync::MutexGuard<'static, ()>, String> {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in profile_id.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    PROFILE_LOCKS[hash as usize % PROFILE_LOCK_SHARDS]
        .lock()
        .map_err(|_| "profile mutation lock is poisoned".to_string())
}

fn lock_asset_cache(request_key: &str) -> Result<std::sync::MutexGuard<'static, ()>, String> {
    let shard = request_key
        .bytes()
        .take(8)
        .fold(0_usize, |hash, byte| hash.wrapping_mul(33) ^ byte as usize)
        % ASSET_CACHE_LOCK_SHARDS;
    ASSET_CACHE_LOCKS[shard]
        .lock()
        .map_err(|_| "asset cache lock is poisoned".to_string())
}
fn lock_all_profile_mutations() -> Result<Vec<std::sync::MutexGuard<'static, ()>>, String> {
    PROFILE_LOCKS
        .iter()
        .map(|lock| {
            lock.lock()
                .map_err(|_| "profile mutation lock is poisoned".to_string())
        })
        .collect()
}

fn lock_all_asset_caches() -> Result<Vec<std::sync::MutexGuard<'static, ()>>, String> {
    ASSET_CACHE_LOCKS
        .iter()
        .map(|lock| {
            lock.lock()
                .map_err(|_| "asset cache lock is poisoned".to_string())
        })
        .collect()
}

fn validate_profile_id(id: &str) -> Result<(), String> {
    profile::validate_profile_id(id).map_err(|error| error.to_string())
}

fn smoke_allows_running() -> bool {
    #[cfg(test)]
    if std::env::var_os("PERFECT_SYNC_SMOKE_ALLOW_RUNNING").as_deref()
        == Some(std::ffi::OsStr::new("1"))
    {
        return true;
    }
    false
}

fn launch_pending(workspace_id: &str) -> Result<bool, String> {
    LAUNCH_PENDING
        .lock()
        .map(|pending| pending.contains(workspace_id))
        .map_err(|_| "launch-session lock is poisoned".to_string())
}

fn workspace_is_stopped(workspace_id: &str) -> Result<(), String> {
    validate_profile_id(workspace_id)?;
    if smoke_allows_running() {
        return Ok(());
    }
    if launch_pending(workspace_id)? {
        return Err(
            "This profile is still launching. Wait for startup to finish before changing its workspace."
                .into(),
        );
    }
    let game_dir = managed_instance::workspace_game_dir(workspace_id)?;
    match process::try_is_game_dir_running(&game_dir) {
        Ok(false) => Ok(()),
        Ok(true) => Err(
            "This profile is running. Use Stop in Perfect Sync, or close its Among Us window, before changing its workspace."
                .into(),
        ),
        Err(error) => Err(format!(
            "Could not verify whether this profile is running; refusing to modify its workspace: {error}"
        )),
    }
}

fn game_path_is_stopped(game_dir: &Path) -> Result<(), String> {
    if smoke_allows_running() {
        return Ok(());
    }
    match process::try_is_game_dir_running(game_dir) {
        Ok(false) => Ok(()),
        Ok(true) => Err("This Among Us instance is running. Close it first.".into()),
        Err(error) => Err(format!(
            "Could not verify whether this Among Us instance is running; refusing to modify it: {error}"
        )),
    }
}

fn all_games_are_stopped() -> Result<(), String> {
    if smoke_allows_running() {
        return Ok(());
    }
    if !LAUNCH_PENDING
        .lock()
        .map_err(|_| "launch-session lock is poisoned".to_string())?
        .is_empty()
    {
        return Err("Among Us is still launching. Wait for startup to finish.".into());
    }
    match process::try_is_running() {
        Ok(false) => Ok(()),
        Ok(true) => Err("Among Us is running. Close every instance first.".into()),
        Err(error) => Err(format!(
            "Could not verify whether Among Us is running; refusing to modify shared save data: {error}"
        )),
    }
}

fn managed_workspaces_are_stopped() -> Result<(), String> {
    if smoke_allows_running() {
        return Ok(());
    }
    if !LAUNCH_PENDING
        .lock()
        .map_err(|_| "launch-session lock is poisoned".to_string())?
        .is_empty()
    {
        return Err("A managed profile is still launching. Wait for startup to finish.".into());
    }
    for workspace_id in managed_instance::workspace_ids()? {
        let game_dir = managed_instance::workspace_game_dir(&workspace_id)?;
        match process::try_is_game_dir_running(&game_dir) {
            Ok(false) => {}
            Ok(true) => {
                return Err(format!(
                    "{workspace_id} is running. Close it before moving Perfect Sync storage."
                ));
            }
            Err(error) => {
                return Err(format!(
                    "Could not verify whether managed profile {workspace_id} is running; refusing to move storage: {error}"
                ));
            }
        }
    }
    Ok(())
}

fn spawn_launch(
    workspace_id: &str,
    operation: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    validate_profile_id(workspace_id)?;
    let game_dir = managed_instance::workspace_game_dir(workspace_id)?;
    {
        let mut pending = LAUNCH_PENDING
            .lock()
            .map_err(|_| "launch-session lock is poisoned".to_string())?;
        if !pending.insert(workspace_id.to_string()) {
            return Err("This profile is already launching.".into());
        }
    }
    log::info!(
        target: "perfect_sync::support",
        "launch attempt started; profile={workspace_id}"
    );
    if let Err(error) = operation() {
        if let Ok(mut pending) = LAUNCH_PENDING.lock() {
            pending.remove(workspace_id);
        }
        log::error!(
            target: "perfect_sync::support",
            "launch attempt failed; profile={workspace_id}; error={}",
            redact_crossover_text(support_log_message(error.clone()))
        );
        return Err(error);
    }
    log::info!(
        target: "perfect_sync::support",
        "launch dispatch completed; profile={workspace_id}"
    );
    let workspace_id = workspace_id.to_string();
    std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let ready = matches!(process::try_is_game_dir_running(&game_dir), Ok(true));
            let timed_out = Instant::now() >= deadline;
            if ready || timed_out {
                if let Ok(mut pending) = LAUNCH_PENDING.lock() {
                    pending.remove(&workspace_id);
                }
                if ready {
                    log::info!(
                        target: "perfect_sync::support",
                        "launch process confirmed; profile={workspace_id}"
                    );
                } else {
                    log::warn!(
                        target: "perfect_sync::support",
                        "launch process was not confirmed within the post-dispatch window; profile={workspace_id}"
                    );
                }
                break;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    });
    Ok(())
}

fn wait_for_crossover_process(
    executable: &Path,
    process_name: &str,
    timeout: Duration,
    stability: Duration,
    mut poll_launcher: impl FnMut() -> Result<(), String>,
) -> Result<bool, String> {
    let deadline = Instant::now() + timeout;
    let mut ready_since = None;
    let mut poll_interval = Duration::from_millis(100);
    loop {
        poll_launcher()?;
        let running = process::try_is_executable_running(executable).map_err(|error| {
            format!("Could not verify whether CrossOver started {process_name}: {error}")
        })?;
        let now = Instant::now();
        if running {
            let ready = ready_since.get_or_insert(now);
            if now.duration_since(*ready) >= stability {
                return Ok(true);
            }
            poll_interval = Duration::from_millis(100);
        } else if ready_since.take().is_some() {
            return Err(format!(
                "{process_name} exited before remaining alive for {} during CrossOver startup.",
                crossover_timeout_label(stability)
            ));
        }
        if now >= deadline {
            return Ok(false);
        }
        std::thread::sleep(poll_interval.min(deadline - now));
        if !running {
            poll_interval = (poll_interval + poll_interval).min(Duration::from_secs(1));
        }
    }
}

#[derive(Default)]
struct CrossoverOutputStream {
    bytes: Vec<u8>,
    truncated: bool,
    closed: bool,
    read_error: Option<String>,
}

struct CrossoverOutputCapture {
    stdout: Option<Arc<Mutex<CrossoverOutputStream>>>,
    stderr: Option<Arc<Mutex<CrossoverOutputStream>>>,
}

impl CrossoverOutputCapture {
    fn new(launcher: &mut std::process::Child) -> Self {
        Self {
            stdout: launcher.stdout.take().map(drain_crossover_output),
            stderr: launcher.stderr.take().map(drain_crossover_output),
        }
    }

    fn snapshot(&self) -> String {
        let mut diagnostic = String::new();
        append_crossover_output(&mut diagnostic, "stdout", self.stdout.as_ref());
        append_crossover_output(&mut diagnostic, "stderr", self.stderr.as_ref());
        diagnostic
    }

    fn finish(self) -> String {
        let deadline = Instant::now() + CROSSOVER_OUTPUT_DRAIN_TIMEOUT;
        while !self.is_closed() {
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(5).min(deadline - now));
        }
        self.snapshot()
    }

    fn is_closed(&self) -> bool {
        [&self.stdout, &self.stderr].into_iter().all(|stream| {
            stream
                .as_ref()
                .is_none_or(|stream| stream.lock().is_ok_and(|stream| stream.closed))
        })
    }
}

fn drain_crossover_output(
    mut reader: impl Read + Send + 'static,
) -> Arc<Mutex<CrossoverOutputStream>> {
    let stream = Arc::new(Mutex::new(CrossoverOutputStream::default()));
    let writer = Arc::clone(&stream);
    std::thread::spawn(move || {
        let mut chunk = [0_u8; 4096];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) => break,
                Ok(read) => {
                    let Ok(mut output) = writer.lock() else {
                        return;
                    };
                    if read >= CROSSOVER_OUTPUT_BYTES_PER_STREAM {
                        output.bytes.clear();
                        output.bytes.extend_from_slice(
                            &chunk[read - CROSSOVER_OUTPUT_BYTES_PER_STREAM..read],
                        );
                        output.truncated = true;
                        continue;
                    }
                    let overflow = (output.bytes.len() + read)
                        .saturating_sub(CROSSOVER_OUTPUT_BYTES_PER_STREAM);
                    if overflow > 0 {
                        output.bytes.drain(..overflow);
                        output.truncated = true;
                    }
                    output.bytes.extend_from_slice(&chunk[..read]);
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    if let Ok(mut output) = writer.lock() {
                        output.read_error = Some(error.to_string());
                    }
                    break;
                }
            }
        }
        if let Ok(mut output) = writer.lock() {
            output.closed = true;
        }
    });
    stream
}

fn redact_user_paths_and_tokens(mut text: String) -> String {
    for variable in ["USERPROFILE", "HOME", "APPDATA", "LOCALAPPDATA"] {
        if let Some(path) = std::env::var_os(variable).filter(|value| !value.is_empty()) {
            text = text.replace(&path.to_string_lossy().to_string(), "<redacted-user-path>");
        }
    }
    SUPPORT_TOKEN_PATTERN
        .replace_all(&text, "<redacted-token>")
        .into_owned()
}

fn redact_crossover_text(text: String) -> String {
    if settings::cache_dir_if_initialized().is_some() {
        if let Ok(saved) = settings::load() {
            return redact_sensitive(text, &saved);
        }
    }
    redact_user_paths_and_tokens(text)
}
fn support_log_message(mut message: String) -> String {
    message = message.replace('\0', "\u{fffd}");
    if message.len() > MAX_SUPPORT_EVENT_BYTES {
        let mut boundary = MAX_SUPPORT_EVENT_BYTES;
        while !message.is_char_boundary(boundary) {
            boundary -= 1;
        }
        message.truncate(boundary);
        message.push_str("\n[message truncated at 64 KiB]");
    }
    redact_user_paths_and_tokens(message)
}

#[tauri::command]
pub fn record_support_event(level: String, message: String) -> Result<(), String> {
    if !settings::support_logging_enabled() {
        return Ok(());
    }
    let level = match level.as_str() {
        "debug" => log::Level::Debug,
        "info" => log::Level::Info,
        "warn" => log::Level::Warn,
        "error" => log::Level::Error,
        _ => return Err("Unsupported support log level.".into()),
    };
    let message = if matches!(level, log::Level::Warn | log::Level::Error) {
        redact_crossover_text(support_log_message(message))
    } else {
        support_log_message(message)
    };
    if !message.trim().is_empty() {
        log::log!(target: "perfect_sync::support", level, "{message}");
    }
    Ok(())
}

fn append_crossover_output(
    diagnostic: &mut String,
    name: &str,
    stream: Option<&Arc<Mutex<CrossoverOutputStream>>>,
) {
    let Some(stream) = stream else {
        return;
    };
    let Ok(stream) = stream.lock() else {
        return;
    };
    let text: String = String::from_utf8_lossy(&stream.bytes)
        .chars()
        .filter(|character| !character.is_control() || matches!(character, '\n' | '\r' | '\t'))
        .collect();
    let text = redact_crossover_text(text);
    let text = text.trim();
    if text.is_empty() && stream.read_error.is_none() {
        return;
    }
    diagnostic.push_str("\nCrossOver ");
    diagnostic.push_str(name);
    diagnostic.push_str(" (recent output, capped at 8 KiB");
    if stream.truncated {
        diagnostic.push_str(", earlier output omitted");
    }
    diagnostic.push_str("): ");
    diagnostic.push_str(text);
    if let Some(error) = &stream.read_error {
        if !text.is_empty() {
            diagnostic.push_str("; ");
        }
        diagnostic.push_str("capture ended with ");
        diagnostic.push_str(error);
    }
}

fn with_crossover_output(mut message: String, diagnostic: String) -> String {
    if !diagnostic.is_empty() {
        message.push_str(&diagnostic);
    }
    message
}

fn crossover_exit_error(status: std::process::ExitStatus, diagnostic: String) -> String {
    let message = if status.success() {
        "CrossOver's attached wrapper exited successfully before the exact managed Among Us process became ready. Verify the selected bottle and CrossOver installation, then retry.".to_string()
    } else {
        format!(
            "CrossOver's attached wrapper exited with {status} before the exact managed Among Us process became ready. Verify the selected bottle and CrossOver installation, then retry."
        )
    };
    with_crossover_output(message, diagnostic)
}

fn crossover_timeout_label(timeout: Duration) -> String {
    if timeout.as_millis() % 60_000 == 0 {
        let minutes = timeout.as_secs() / 60;
        format!(
            "{minutes} {}",
            if minutes == 1 { "minute" } else { "minutes" }
        )
    } else if timeout.as_millis() % 1_000 == 0 {
        let seconds = timeout.as_secs();
        format!(
            "{seconds} {}",
            if seconds == 1 { "second" } else { "seconds" }
        )
    } else {
        format!("{} ms", timeout.as_millis())
    }
}

fn launch_crossover(
    specification: &process::LaunchSpec,
    interactive: bool,
) -> io::Result<std::process::Child> {
    if let Some(message) = &specification.error {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, message.clone()));
    }
    let mut command = if interactive {
        process::interactive_command(&specification.program)
    } else {
        process::command(&specification.program)
    };
    command
        .current_dir(&specification.cwd)
        .stdin(if interactive {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .args(&specification.args);
    for (key, value) in &specification.env {
        command.env(key, value);
    }
    command.spawn()
}

fn crossover_steam_client(
    source_game_dir: &Path,
    context: &compat::RuntimeContext,
) -> Result<PathBuf, String> {
    let prefix = context
        .prefix
        .as_deref()
        .ok_or("The selected CrossOver Steam source has no bottle prefix.")?;
    let mut candidates = Vec::with_capacity(3);
    if let Some(steamapps) = source_game_dir.ancestors().find(|path| {
        path.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.eq_ignore_ascii_case("steamapps"))
    }) {
        if let Some(steam_root) = steamapps.parent() {
            candidates.push(steam_root.join("steam.exe"));
        }
    }
    candidates.extend([
        prefix.join("drive_c/Program Files (x86)/Steam/steam.exe"),
        prefix.join("drive_c/Program Files/Steam/steam.exe"),
    ]);
    candidates
        .into_iter()
        .find(|candidate| candidate.is_file())
        .ok_or_else(|| {
            "Could not find steam.exe in the selected CrossOver bottle. Repair the bottle's Steam installation, then retry."
                .to_string()
        })
}

fn crossover_steam_launch_spec(
    steam_client: &Path,
    context: &compat::RuntimeContext,
) -> Result<process::LaunchSpec, String> {
    let program = context.launcher.clone().ok_or(
        "Could not find CrossOver's command-line Wine launcher. Install CrossOver in the system or user Applications folder, then retry.",
    )?;
    let mut args = context.launcher_args.clone();
    let delimiter = args
        .iter()
        .position(|argument| argument == "--")
        .unwrap_or(args.len());
    args.splice(
        delimiter..delimiter,
        [
            OsString::from("--no-update"),
            OsString::from("--no-gui"),
            OsString::from("--wait-children"),
        ],
    );
    args.extend([
        steam_client.as_os_str().to_owned(),
        OsString::from("-silent"),
    ]);
    let mut env = Vec::new();
    if let Some(bottle) = context
        .prefix
        .as_deref()
        .and_then(Path::file_name)
        .map(OsString::from)
    {
        env.push((OsString::from("CX_BOTTLE"), bottle));
    }
    Ok(process::LaunchSpec {
        program,
        args,
        cwd: steam_client
            .parent()
            .ok_or("CrossOver's Steam executable has no parent folder.")?
            .to_path_buf(),
        env,
        error: None,
    })
}

fn poll_crossover_steam_wrapper(
    launcher: &mut std::process::Child,
    wrapper_exit: &mut Option<std::process::ExitStatus>,
) -> Result<(), String> {
    if let Some(status) = wrapper_exit.as_ref() {
        return if status.success() {
            Ok(())
        } else {
            Err(format!(
                "CrossOver's Steam wrapper exited with {status} before Steam became ready."
            ))
        };
    }
    *wrapper_exit = launcher
        .try_wait()
        .map_err(|error| format!("Could not verify CrossOver's Steam startup: {error}"))?;
    match wrapper_exit.as_ref() {
        Some(status) if !status.success() => Err(format!(
            "CrossOver's Steam wrapper exited with {status} before Steam became ready."
        )),
        _ => Ok(()),
    }
}

fn ensure_crossover_steam_ready(
    source_game_dir: &Path,
    context: &compat::RuntimeContext,
) -> Result<(), String> {
    let steam_client = crossover_steam_client(source_game_dir, context)?;
    if process::try_is_executable_running(&steam_client)
        .map_err(|error| format!("Could not check CrossOver's Steam client: {error}"))?
    {
        log::info!(
            target: "perfect_sync::support",
            "CrossOver Steam client already running"
        );
        return Ok(());
    }

    let specification = crossover_steam_launch_spec(&steam_client, context)?;
    let mut launcher = launch_crossover(&specification, false).map_err(|error| {
        format!("Could not start Steam in the selected CrossOver bottle: {error}")
    })?;
    let wrapper_pid = launcher.id();
    log::info!(
        target: "perfect_sync::support",
        "CrossOver Steam startup started; wrapper_pid={wrapper_pid}; timeout_ms={}",
        CROSSOVER_STEAM_START_TIMEOUT.as_millis()
    );
    let output = CrossoverOutputCapture::new(&mut launcher);
    let mut wrapper_exit = None;
    let ready = wait_for_crossover_process(
        &steam_client,
        "Steam",
        CROSSOVER_STEAM_START_TIMEOUT,
        CROSSOVER_STEAM_STABILITY,
        || poll_crossover_steam_wrapper(&mut launcher, &mut wrapper_exit),
    );

    match ready {
        Ok(true) => {
            let startup_diagnostic = output.snapshot();
            if !startup_diagnostic.is_empty() {
                log::debug!(
                    target: "perfect_sync::support",
                    "CrossOver Steam startup output: {}",
                    redact_crossover_text(support_log_message(startup_diagnostic))
                );
            }
            let observed_exit = match wrapper_exit.take() {
                Some(status) => Some(status),
                None => launcher.try_wait().map_err(|error| {
                    format!("Could not verify CrossOver's Steam startup: {error}")
                })?,
            };
            match observed_exit {
                Some(status) if !status.success() => Err(with_crossover_output(
                    format!(
                        "CrossOver's Steam wrapper exited with {status} after Steam readiness."
                    ),
                    output.finish(),
                )),
                Some(status) => {
                    let diagnostic = output.finish();
                    log::info!(
                        target: "perfect_sync::support",
                        "CrossOver Steam client ready; wrapper_pid={wrapper_pid}; status={status}"
                    );
                    if !diagnostic.is_empty() {
                        log::debug!(
                            target: "perfect_sync::support",
                            "CrossOver Steam final output: {}",
                            redact_crossover_text(support_log_message(diagnostic))
                        );
                    }
                    Ok(())
                }
                None => {
                    std::thread::spawn(move || {
                        match launcher.wait() {
                            Ok(status) => log::info!(
                                target: "perfect_sync::support",
                                "CrossOver Steam wrapper exited; wrapper_pid={wrapper_pid}; status={status}"
                            ),
                            Err(error) => log::warn!(
                                target: "perfect_sync::support",
                                "Could not reap CrossOver's Steam wrapper; wrapper_pid={wrapper_pid}; error={error}"
                            ),
                        }
                        let diagnostic = output.finish();
                        if !diagnostic.is_empty() {
                            log::debug!(
                                target: "perfect_sync::support",
                                "CrossOver Steam final output: {}",
                                redact_crossover_text(support_log_message(diagnostic))
                            );
                        }
                    });
                    log::info!(
                        target: "perfect_sync::support",
                        "CrossOver Steam client ready; wrapper_pid={wrapper_pid}"
                    );
                    Ok(())
                }
            }
        }
        Ok(false) => {
            let wrapper_evidence = wrapper_exit
                .as_ref()
                .map(|status| format!(" CrossOver's Steam wrapper exited with {status}."))
                .unwrap_or_default();
            let cleanup_result = stop_crossover_launcher(&mut launcher);
            let message = with_crossover_output(
                format!(
                    "Steam did not become ready in the selected CrossOver bottle within {}.{wrapper_evidence}",
                    crossover_timeout_label(CROSSOVER_STEAM_START_TIMEOUT)
                ),
                output.finish(),
            );
            cleanup_result.map_err(|error| format!("{message} {error}"))?;
            Err(message)
        }
        Err(error) => {
            let cleanup_result = stop_crossover_launcher(&mut launcher);
            let message = with_crossover_output(error, output.finish());
            cleanup_result.map_err(|cleanup_error| format!("{message} {cleanup_error}"))?;
            Err(message)
        }
    }
}

fn submit_crossover_enter(launcher: &mut std::process::Child) -> Result<(), String> {
    let Some(mut input) = launcher.stdin.take() else {
        return Ok(());
    };
    match input.write_all(b"\n") {
        Ok(()) => input
            .flush()
            .map_err(|error| format!("Could not finish CrossOver helper input: {error}")),
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        Err(error) => Err(format!("Could not submit CrossOver helper input: {error}")),
    }
}

fn stop_crossover_launcher(launcher: &mut std::process::Child) -> Result<(), String> {
    if launcher
        .try_wait()
        .map_err(|error| format!("Could not verify CrossOver's launch result: {error}"))?
        .is_some()
    {
        return Ok(());
    }
    launcher.kill().map_err(|error| {
        format!("CrossOver's launcher was still running and could not be closed: {error}")
    })?;
    launcher
        .wait()
        .map_err(|error| format!("Could not finish closing CrossOver's launcher: {error}"))?;
    Ok(())
}
fn stop_crossover_attempt(
    launcher: &mut std::process::Child,
    game_dir: &Path,
) -> Result<bool, String> {
    let input_result = submit_crossover_enter(launcher);
    let launcher_result = stop_crossover_launcher(launcher);
    let game_result = process::terminate_game_dir(game_dir).map_err(|error| {
        format!("Could not stop the exact managed CrossOver process after launch failure: {error}")
    });
    match (input_result, launcher_result, game_result) {
        (Ok(()), Ok(()), Ok(stopped)) => Ok(stopped),
        (input, launcher, game) => {
            let errors = [input.err(), launcher.err(), game.err()]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>();
            Err(errors.join(". "))
        }
    }
}

fn supervise_crossover_launch(
    mut launcher: std::process::Child,
    game_dir: &Path,
    timeout: Duration,
) -> Result<(), String> {
    let started = Instant::now();
    log::info!(
        target: "perfect_sync::support",
        "CrossOver supervision started; wrapper_pid={}; timeout_ms={}",
        launcher.id(),
        timeout.as_millis()
    );
    let output = CrossoverOutputCapture::new(&mut launcher);
    let mut wrapper_exit = None;
    let mut wrapper_observed_alive = false;
    let launch_result = wait_for_crossover_process(
        &game_dir.join(process::GAME_EXE),
        "Among Us",
        timeout,
        CROSSOVER_GAME_STABILITY,
        || {
            wrapper_exit = launcher
                .try_wait()
                .map_err(|error| format!("Could not verify CrossOver's launch result: {error}"))?;
            if wrapper_exit.is_some() {
                Err("CrossOver's attached wrapper exited during startup.".into())
            } else {
                wrapper_observed_alive = true;
                Ok(())
            }
        },
    );

    match launch_result {
        Ok(true) => {
            if let Err(error) = submit_crossover_enter(&mut launcher) {
                let cleanup_error = stop_crossover_attempt(&mut launcher, game_dir).err();
                let message = with_crossover_output(
                    format!("Among Us started, but Perfect Sync could not release CrossOver's interactive helper: {error}"),
                    output.finish(),
                );
                return Err(match cleanup_error {
                    Some(cleanup_error) => format!("{message} {cleanup_error}"),
                    None => message,
                });
            }
            let startup_diagnostic = output.snapshot();
            if !startup_diagnostic.is_empty() {
                log::debug!(
                    target: "perfect_sync::support",
                    "CrossOver startup output: {}",
                    redact_crossover_text(support_log_message(startup_diagnostic))
                );
            }
            let wrapper_pid = launcher.id();
            match launcher
                .try_wait()
                .map_err(|error| format!("Could not verify CrossOver's launch result: {error}"))?
            {
                Some(status) if !status.success() => {
                    let error = crossover_exit_error(status, output.finish());
                    log::error!(
                        target: "perfect_sync::support",
                        "CrossOver supervision failed after readiness: {}",
                        redact_crossover_text(support_log_message(error.clone()))
                    );
                    return Err(error);
                }
                Some(status) => {
                    let diagnostic = output.finish();
                    log::info!(
                        target: "perfect_sync::support",
                        "CrossOver wrapper exited after readiness; wrapper_pid={wrapper_pid}; status={status}"
                    );
                    if !diagnostic.is_empty() {
                        log::debug!(
                            target: "perfect_sync::support",
                            "CrossOver final output: {}",
                            redact_crossover_text(support_log_message(diagnostic))
                        );
                    }
                }
                None => {
                    std::thread::spawn(move || {
                        match launcher.wait() {
                            Ok(status) => log::info!(
                                target: "perfect_sync::support",
                                "CrossOver wrapper exited; wrapper_pid={wrapper_pid}; status={status}"
                            ),
                            Err(error) => log::warn!(
                                target: "perfect_sync::support",
                                "Could not reap CrossOver's attached wrapper; wrapper_pid={wrapper_pid}; error={error}"
                            ),
                        }
                        let diagnostic = output.finish();
                        if !diagnostic.is_empty() {
                            log::debug!(
                                target: "perfect_sync::support",
                                "CrossOver final output: {}",
                                redact_crossover_text(support_log_message(diagnostic))
                            );
                        }
                    });
                }
            }
            log::info!(
                target: "perfect_sync::support",
                "CrossOver process confirmed after {} ms",
                started.elapsed().as_millis()
            );
            Ok(())
        }
        Ok(false) => {
            if let Some(status) = launcher
                .try_wait()
                .map_err(|error| format!("Could not verify CrossOver's launch result: {error}"))?
            {
                let cleanup_result = stop_crossover_attempt(&mut launcher, game_dir);
                let message = crossover_exit_error(status, output.finish());
                cleanup_result.map_err(|error| format!("{message} {error}"))?;
                return Err(message);
            }
            let cleanup_result = stop_crossover_attempt(&mut launcher, game_dir);
            let scoped_result = match &cleanup_result {
                Ok(true) => "The exact managed process was stopped.",
                Ok(false) => "No exact managed process remained.",
                Err(_) => "Perfect Sync could not verify complete launch cleanup.",
            };
            let message = with_crossover_output(
                format!(
                    "Perfect Sync did not detect the exact managed Among Us process within {}. CrossOver's attached wrapper remained alive until the readiness timeout and was stopped. {scoped_result} The launch is no longer pending; retry it once.",
                    crossover_timeout_label(timeout)
                ),
                output.finish(),
            );
            cleanup_result.map_err(|error| format!("{message} {error}"))?;
            Err(message)
        }
        Err(error) => {
            if let Some(status) = wrapper_exit {
                let cleanup_result = stop_crossover_attempt(&mut launcher, game_dir);
                let message = crossover_exit_error(status, output.finish());
                cleanup_result.map_err(|cleanup_error| format!("{message} {cleanup_error}"))?;
                return Err(message);
            }
            let wrapper_evidence = if wrapper_observed_alive {
                " CrossOver's attached wrapper remained alive when readiness checking failed."
            } else {
                " CrossOver's attached wrapper state could not be verified."
            };
            let cleanup_result = stop_crossover_attempt(&mut launcher, game_dir);
            let message =
                with_crossover_output(format!("{error}{wrapper_evidence}"), output.finish());
            cleanup_result.map_err(|cleanup_error| format!("{message} {cleanup_error}"))?;
            Err(message)
        }
    }
}

fn unique_sibling(path: &Path, label: &str) -> Result<PathBuf, String> {
    let parent = path.parent().ok_or("path has no parent directory")?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("path has no portable file name")?;
    for _ in 0..128 {
        let serial = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let candidate = parent.join(format!(".{name}.{label}.{}.{}", std::process::id(), serial));
        match fs::symlink_metadata(&candidate) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(candidate),
            Ok(_) => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    Err("could not allocate a unique temporary path".into())
}

#[cfg(windows)]
fn atomic_replace_file(source: &Path, destination: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    extern "system" {
        fn MoveFileExW(existing: *const u16, new: *const u16, flags: u32) -> i32;
    }
    let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;
    let result = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn atomic_replace_file(source: &Path, destination: &Path) -> io::Result<()> {
    fs::rename(source, destination)
}

fn read_bounded(path: &Path, limit: u64) -> Result<Option<Vec<u8>>, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    if is_reparse(&metadata) || !metadata.is_file() {
        return Err(format!("{} is not a regular file", path.display()));
    }
    if metadata.len() > limit {
        return Err(format!("{} exceeds its size limit", path.display()));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    File::open(path)
        .map_err(|error| error.to_string())?
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 > limit {
        return Err(format!("{} exceeds its size limit", path.display()));
    }
    Ok(Some(bytes))
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path.parent().ok_or("file has no parent directory")?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let temporary = unique_sibling(path, "tmp")?;
    let result = (|| {
        let mut output = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .map_err(|error| error.to_string())?;
        output.write_all(bytes).map_err(|error| error.to_string())?;
        output.sync_all().map_err(|error| error.to_string())?;
        drop(output);
        atomic_replace_file(&temporary, path).map_err(|error| error.to_string())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(windows)]
fn is_reparse(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_attributes() & 0x400 != 0
}

#[cfg(not(windows))]
fn is_reparse(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

fn copy_profile_tree(source: &Path, destination: &Path) -> Result<(), String> {
    let started = Instant::now();
    let source_metadata = fs::symlink_metadata(source).map_err(|error| error.to_string())?;
    if is_reparse(&source_metadata) || !source_metadata.is_dir() {
        return Err("profile is not a regular directory".into());
    }
    fs::create_dir(destination).map_err(|error| error.to_string())?;
    let mut pending = vec![(source.to_path_buf(), destination.to_path_buf())];
    let mut files = 0_usize;
    let mut bytes = 0_u64;
    while let Some((from, to)) = pending.pop() {
        for entry in fs::read_dir(&from).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            let metadata = fs::symlink_metadata(entry.path()).map_err(|error| error.to_string())?;
            if is_reparse(&metadata) {
                return Err(format!(
                    "profile contains a link or reparse point: {}",
                    entry.path().display()
                ));
            }
            let target = to.join(entry.file_name());
            if metadata.is_dir() {
                fs::create_dir(&target).map_err(|error| error.to_string())?;
                pending.push((entry.path(), target));
            } else if metadata.is_file() {
                files += 1;
                bytes = bytes
                    .checked_add(metadata.len())
                    .filter(|total| *total <= MAX_PROFILE_STAGE_BYTES)
                    .ok_or("profile exceeds the staging byte limit")?;
                if files > MAX_PROFILE_STAGE_FILES {
                    return Err("profile contains too many files".into());
                }
                let mut input = File::open(entry.path()).map_err(|error| error.to_string())?;
                let mut output = OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .open(&target)
                    .map_err(|error| error.to_string())?;
                let copied =
                    io::copy(&mut input, &mut output).map_err(|error| error.to_string())?;
                if copied != metadata.len() {
                    return Err("profile file changed while it was staged".into());
                }
                output.sync_all().map_err(|error| error.to_string())?;
            } else {
                return Err("profile contains an unsupported filesystem entry".into());
            }
        }
    }
    log_perf("profile_tree_copy", started, files, bytes);
    Ok(())
}

/// Build a transaction view without copying immutable payload bytes. Every profile
/// mutation used by this module replaces, renames, or removes files; it never
/// truncates an existing payload in place, so hard-linked inputs remain unchanged.
fn stage_profile_tree(source: &Path, destination: &Path) -> Result<(), String> {
    let started = Instant::now();
    let source_metadata = fs::symlink_metadata(source).map_err(|error| error.to_string())?;
    if is_reparse(&source_metadata) || !source_metadata.is_dir() {
        return Err("profile is not a regular directory".into());
    }
    fs::create_dir(destination).map_err(|error| error.to_string())?;
    let mut pending = vec![(source.to_path_buf(), destination.to_path_buf())];
    let mut files = 0_usize;
    let mut bytes = 0_u64;
    let mut copied_bytes = 0_u64;
    while let Some((from, to)) = pending.pop() {
        for entry in fs::read_dir(&from).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            let metadata = fs::symlink_metadata(entry.path()).map_err(|error| error.to_string())?;
            if is_reparse(&metadata) {
                return Err(format!(
                    "profile contains a link or reparse point: {}",
                    entry.path().display()
                ));
            }
            let target = to.join(entry.file_name());
            if metadata.is_dir() {
                fs::create_dir(&target).map_err(|error| error.to_string())?;
                pending.push((entry.path(), target));
                continue;
            }
            if !metadata.is_file() {
                return Err("profile contains an unsupported filesystem entry".into());
            }
            files += 1;
            bytes = bytes
                .checked_add(metadata.len())
                .filter(|total| *total <= MAX_PROFILE_STAGE_BYTES)
                .ok_or("profile exceeds the staging byte limit")?;
            if files > MAX_PROFILE_STAGE_FILES {
                return Err("profile contains too many files".into());
            }
            if fs::hard_link(entry.path(), &target).is_err() {
                let mut input = File::open(entry.path()).map_err(|error| error.to_string())?;
                let mut output = OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .open(&target)
                    .map_err(|error| error.to_string())?;
                let copied =
                    io::copy(&mut input, &mut output).map_err(|error| error.to_string())?;
                if copied != metadata.len() {
                    return Err("profile file changed while it was staged".into());
                }
                output.sync_all().map_err(|error| error.to_string())?;
                copied_bytes += copied;
            }
        }
    }
    log::info!(
        target: "perfect_sync::performance",
        "profile_tree_stage completed in {} ms ({files} files, {bytes} logical bytes, {copied_bytes} copied bytes)",
        started.elapsed().as_millis()
    );
    Ok(())
}

const DISABLED_DOORSTOP: &str = ".perfectsync-winhttp.disabled";
const APP_LOADER_MARKER: &str = ".perfectsync_loader";

fn copy_snapshot_path(source: &Path, destination: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(source).map_err(|error| error.to_string())?;
    if is_reparse(&metadata) {
        return Err(format!("{} is a link or reparse point", source.display()));
    }
    if metadata.is_dir() {
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        copy_profile_tree(source, destination)
    } else if metadata.is_file() {
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let mut input = File::open(source).map_err(|error| error.to_string())?;
        let mut output = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(destination)
            .map_err(|error| error.to_string())?;
        let copied = io::copy(&mut input, &mut output).map_err(|error| error.to_string())?;
        if copied != metadata.len() {
            return Err(format!(
                "{} changed while it was snapshotted",
                source.display()
            ));
        }
        output.sync_all().map_err(|error| error.to_string())
    } else {
        Err(format!(
            "{} is not a regular file or directory",
            source.display()
        ))
    }
}

fn remove_snapshot_target(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if is_reparse(&metadata) => Err(format!(
            "refusing to replace reparse point {}",
            path.display()
        )),
        Ok(metadata) if metadata.is_dir() => {
            fs::remove_dir_all(path).map_err(|error| error.to_string())
        }
        Ok(metadata) if metadata.is_file() => {
            fs::remove_file(path).map_err(|error| error.to_string())
        }
        Ok(_) => Err(format!(
            "{} is an unsupported filesystem entry",
            path.display()
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

fn game_artifact_transaction<T>(
    game_dir: &Path,
    operation: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let relative_paths = [
        PathBuf::from("winhttp.dll"),
        PathBuf::from(DISABLED_DOORSTOP),
        PathBuf::from("doorstop_config.ini"),
        PathBuf::from(".doorstop_version"),
        PathBuf::from("changelog.txt"),
        PathBuf::from("steam_appid.txt"),
        PathBuf::from("dotnet"),
        PathBuf::from("BepInEx").join(APP_LOADER_MARKER),
        PathBuf::from("BepInEx").join(loader::DOORSTOP_PATCH_MARKER),
        PathBuf::from("BepInEx").join(loader::MANAGED_TOU_PACKAGE_MARKER),
        PathBuf::from(loader::DOORSTOP_PATCH_TRANSACTION),
        PathBuf::from("BepInEx/core"),
        PathBuf::from("BepInEx/config"),
        PathBuf::from("BepInEx/patchers"),
        PathBuf::from("BepInEx/interop"),
        PathBuf::from("BepInEx/unity-libs"),
        PathBuf::from("BepInEx/cache"),
        PathBuf::from("BepInEx").join(loader::UNMANAGED_QUARANTINE_DIR),
        PathBuf::from("BepInEx/plugins"),
    ];
    let backup = unique_sibling(game_dir, "prepare-backup")?;
    fs::create_dir(&backup).map_err(|error| error.to_string())?;
    let bep_existed = game_dir.join("BepInEx").exists();
    for relative in &relative_paths {
        let source = game_dir.join(relative);
        match fs::symlink_metadata(&source) {
            Ok(_) => {
                if let Err(error) = copy_snapshot_path(&source, &backup.join(relative)) {
                    let _ = fs::remove_dir_all(&backup);
                    return Err(error);
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                let _ = fs::remove_dir_all(&backup);
                return Err(error.to_string());
            }
        }
    }
    match operation() {
        Ok(value) => {
            let _ = fs::remove_dir_all(&backup);
            Ok(value)
        }
        Err(error) => {
            let mut rollback_errors = Vec::new();
            for relative in relative_paths.iter().rev() {
                let target = game_dir.join(relative);
                if let Err(rollback) = remove_snapshot_target(&target) {
                    rollback_errors.push(rollback);
                    continue;
                }
                let saved = backup.join(relative);
                if saved.exists() {
                    if let Some(parent) = target.parent() {
                        if let Err(rollback) = fs::create_dir_all(parent) {
                            rollback_errors.push(rollback.to_string());
                            continue;
                        }
                    }
                    if let Err(rollback) = fs::rename(&saved, &target) {
                        rollback_errors.push(rollback.to_string());
                    }
                }
            }
            if !bep_existed {
                match fs::remove_dir(game_dir.join("BepInEx")) {
                    Ok(()) => {}
                    Err(rollback) if rollback.kind() == io::ErrorKind::NotFound => {}
                    Err(rollback) => rollback_errors.push(rollback.to_string()),
                }
            }
            if rollback_errors.is_empty() {
                let _ = fs::remove_dir_all(&backup);
                Err(error)
            } else {
                Err(format!(
                    "{error}; additionally failed to restore game artifacts: {}. Recovery backup retained at {}",
                    rollback_errors.join("; "),
                    backup.display()
                ))
            }
        }
    }
}

fn with_profile_layout<T>(
    profiles_root: &Path,
    profile_id: &str,
    operation: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    recover_profile_transactions(profiles_root)?;
    let bep = profiles_root.join(profile_id).join("BepInEx");
    let plugins = bep.join("plugins");
    let config = bep.join("config");
    let bep_existed = bep.exists();
    let plugins_existed = plugins.exists();
    let config_existed = config.exists();
    let outcome = match loader::ensure_profile_layout(profiles_root, profile_id) {
        Ok(()) => operation(),
        Err(error) => Err(error.to_string()),
    };
    match outcome {
        Ok(value) => Ok(value),
        Err(error) => {
            let mut rollback_errors = Vec::new();
            for (path, existed) in [(&plugins, plugins_existed), (&config, config_existed)] {
                if !existed {
                    if let Err(rollback) = fs::remove_dir(path) {
                        if rollback.kind() != io::ErrorKind::NotFound {
                            rollback_errors.push(rollback.to_string());
                        }
                    }
                }
            }
            if !bep_existed {
                if let Err(rollback) = fs::remove_dir(&bep) {
                    if rollback.kind() != io::ErrorKind::NotFound {
                        rollback_errors.push(rollback.to_string());
                    }
                }
            }
            if rollback_errors.is_empty() {
                Err(error)
            } else {
                Err(format!(
                    "{error}; additionally could not remove the newly created profile layout: {}",
                    rollback_errors.join("; ")
                ))
            }
        }
    }
}

fn with_existing_profile_layout<T>(
    profiles_root: &Path,
    profile_id: &str,
    operation: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    validate_profile_id(profile_id)?;
    recovered_profile_store(profiles_root)?
        .load(profile_id)
        .map_err(|error| error.to_string())?
        .ok_or("profile not found")?;
    with_profile_layout(profiles_root, profile_id, operation)
}

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum ProfileRecoveryAction {
    Publish,
    Rollback,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ProfileRecoveryJournal {
    version: u32,
    profile_id: String,
    action: ProfileRecoveryAction,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ModToggleRecoveryJournal {
    version: u32,
    profile_id: String,
    file: String,
}

fn mod_toggle_recovery_path(root: &Path, id: &str) -> Result<PathBuf, String> {
    validate_profile_id(id)?;
    let parent = root
        .parent()
        .ok_or("profile root has no parent directory")?;
    Ok(parent.join(format!(
        "{}{id}",
        profile_sibling_prefix(root, "mod-toggle")?
    )))
}

fn recover_mod_toggle_transaction(root: &Path, id: &str) -> Result<(), String> {
    let journal_path = mod_toggle_recovery_path(root, id)?;
    let metadata = match fs::symlink_metadata(&journal_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
    };
    if is_reparse(&metadata) || !metadata.is_file() {
        return Err(format!(
            "mod toggle recovery marker is not a regular file: {}",
            journal_path.display()
        ));
    }
    let bytes = read_bounded(&journal_path, MAX_PROFILE_RECOVERY_JOURNAL_BYTES)?
        .ok_or("mod toggle recovery marker disappeared")?;
    let journal: ModToggleRecoveryJournal = serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid mod toggle recovery marker: {error}"))?;
    if journal.version != 1 || journal.profile_id != id {
        return Err("mod toggle recovery marker does not match its profile".into());
    }
    let record = ProfileStore::new(root)
        .load(id)
        .map_err(|error| error.to_string())?
        .ok_or("profile with a pending mod toggle is unavailable")?;
    let enabled = record
        .mods
        .iter()
        .find(|installed| {
            installed
                .file
                .as_deref()
                .is_some_and(|file| file.eq_ignore_ascii_case(&journal.file))
        })
        .map(|installed| installed.enabled)
        .ok_or("mod toggle recovery marker refers to an unknown plugin")?;
    profile::set_plugin_enabled(root, id, &journal.file, enabled).map_err(|error| {
        format!(
            "could not recover the pending mod toggle ({error}); recovery evidence was retained at {}",
            journal_path.display()
        )
    })?;
    fs::remove_file(&journal_path).map_err(|error| {
        format!(
            "mod toggle recovered but its recovery marker could not be removed ({error}): {}",
            journal_path.display()
        )
    })
}

fn write_mod_toggle_recovery_journal(root: &Path, id: &str, file: &str) -> Result<PathBuf, String> {
    let path = mod_toggle_recovery_path(root, id)?;
    let mut bytes = serde_json::to_vec(&ModToggleRecoveryJournal {
        version: 1,
        profile_id: id.to_string(),
        file: file.to_string(),
    })
    .map_err(|error| error.to_string())?;
    bytes.push(b'\n');
    if bytes.len() as u64 > MAX_PROFILE_RECOVERY_JOURNAL_BYTES {
        return Err("mod toggle recovery journal exceeds its size limit".into());
    }
    atomic_write(&path, &bytes)?;
    Ok(path)
}

struct ProfileTransactionPaths {
    stage_root: PathBuf,
    backup_root: PathBuf,
    journal: PathBuf,
}

fn profile_sibling_prefix(root: &Path, label: &str) -> Result<String, String> {
    let name = root
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("profile root has no portable file name")?;
    Ok(format!(".{name}.{label}."))
}

fn profile_transaction_paths(root: &Path, suffix: &str) -> Result<ProfileTransactionPaths, String> {
    let parent = root
        .parent()
        .ok_or("profile root has no parent directory")?;
    Ok(ProfileTransactionPaths {
        stage_root: parent.join(format!(
            "{}{suffix}",
            profile_sibling_prefix(root, "transaction")?
        )),
        backup_root: parent.join(format!(
            "{}{suffix}",
            profile_sibling_prefix(root, "backup")?
        )),
        journal: parent.join(format!(
            "{}{suffix}",
            profile_sibling_prefix(root, "recovery")?
        )),
    })
}

fn valid_profile_transaction_suffix(suffix: &str) -> bool {
    if suffix.is_empty() || suffix.len() > 48 {
        return false;
    }
    let mut parts = suffix.split('.');
    matches!(
        (parts.next(), parts.next(), parts.next()),
        (Some(process), Some(serial), None)
            if !process.is_empty()
                && !serial.is_empty()
                && process.bytes().all(|byte| byte.is_ascii_digit())
                && serial.bytes().all(|byte| byte.is_ascii_digit())
    )
}

fn allocate_profile_transaction_paths(root: &Path) -> Result<ProfileTransactionPaths, String> {
    let stage_prefix = profile_sibling_prefix(root, "transaction")?;
    for _ in 0..128 {
        let stage_root = unique_sibling(root, "transaction")?;
        let stage_name = stage_root
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or("transaction path has no portable file name")?;
        let suffix = stage_name
            .strip_prefix(&stage_prefix)
            .ok_or("transaction path has an invalid name")?;
        let paths = profile_transaction_paths(root, suffix)?;
        let available = [&paths.backup_root, &paths.journal].iter().all(|path| {
            matches!(
                fs::symlink_metadata(path),
                Err(error) if error.kind() == io::ErrorKind::NotFound
            )
        });
        if available {
            return Ok(paths);
        }
    }
    Err("could not allocate profile transaction artifacts".into())
}

fn write_profile_recovery_journal(
    path: &Path,
    profile_id: &str,
    action: ProfileRecoveryAction,
) -> Result<(), String> {
    let mut bytes = serde_json::to_vec(&ProfileRecoveryJournal {
        version: 1,
        profile_id: profile_id.to_string(),
        action,
    })
    .map_err(|error| error.to_string())?;
    bytes.push(b'\n');
    if bytes.len() as u64 > MAX_PROFILE_RECOVERY_JOURNAL_BYTES {
        return Err("profile recovery journal exceeds its size limit".into());
    }
    atomic_write(path, &bytes)
}

fn validate_profile_tree(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if is_reparse(&metadata) || !metadata.is_dir() {
        return Err("profile recovery artifact is not a regular directory".into());
    }
    let mut pending = vec![path.to_path_buf()];
    let mut files = 0_usize;
    let mut bytes = 0_u64;
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            let metadata = fs::symlink_metadata(entry.path()).map_err(|error| error.to_string())?;
            if is_reparse(&metadata) {
                return Err(format!(
                    "profile recovery artifact contains a link or reparse point: {}",
                    entry.path().display()
                ));
            }
            if metadata.is_dir() {
                pending.push(entry.path());
            } else if metadata.is_file() {
                files += 1;
                bytes = bytes
                    .checked_add(metadata.len())
                    .filter(|total| *total <= MAX_PROFILE_STAGE_BYTES)
                    .ok_or("profile recovery artifact exceeds the byte limit")?;
                if files > MAX_PROFILE_STAGE_FILES {
                    return Err("profile recovery artifact contains too many files".into());
                }
            } else {
                return Err("profile recovery artifact contains an unsupported entry".into());
            }
        }
    }
    Ok(())
}

fn validate_recovery_profile(container: &Path, id: &str) -> Result<bool, String> {
    let profile_dir = container.join(id);
    match fs::symlink_metadata(&profile_dir) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.to_string()),
        Ok(_) => {
            validate_profile_tree(&profile_dir)?;
            ProfileStore::new(container)
                .load(id)
                .map_err(|error| error.to_string())?
                .ok_or("profile recovery artifact has no manifest")?;
            Ok(true)
        }
    }
}

fn validate_recovery_container(container: &Path, id: &str) -> Result<(bool, bool), String> {
    let metadata = match fs::symlink_metadata(container) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok((false, false)),
        Err(error) => return Err(error.to_string()),
        Ok(metadata) => metadata,
    };
    if is_reparse(&metadata) || !metadata.is_dir() {
        return Err(format!(
            "{} is not a regular recovery directory",
            container.display()
        ));
    }
    let mut profile_present = false;
    for entry in fs::read_dir(container).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        if entry.file_name() != std::ffi::OsStr::new(id) || profile_present {
            return Err(format!(
                "{} contains ambiguous recovery data",
                container.display()
            ));
        }
        profile_present = true;
    }
    if profile_present {
        validate_recovery_profile(container, id)?;
    }
    Ok((true, profile_present))
}

fn remove_profile_recovery_artifacts(paths: &ProfileTransactionPaths) -> Result<(), String> {
    for directory in [&paths.stage_root, &paths.backup_root] {
        match fs::remove_dir_all(directory) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    fs::remove_file(&paths.journal).map_err(|error| error.to_string())
}

fn recover_profile_transaction(
    root: &Path,
    paths: &ProfileTransactionPaths,
    journal: &ProfileRecoveryJournal,
) -> Result<(), String> {
    for _ in 0..4 {
        let final_present = validate_recovery_profile(root, &journal.profile_id)?;
        let (_, stage_present) =
            validate_recovery_container(&paths.stage_root, &journal.profile_id)?;
        let (_, backup_present) =
            validate_recovery_container(&paths.backup_root, &journal.profile_id)?;
        match journal.action {
            ProfileRecoveryAction::Publish => {
                match (final_present, stage_present, backup_present) {
                    (true, true, true) => {
                        return Err("both final, staged, and backup profiles exist".into())
                    }
                    (true, true, false) => {
                        fs::rename(
                            root.join(&journal.profile_id),
                            paths.backup_root.join(&journal.profile_id),
                        )
                        .map_err(|error| error.to_string())?;
                    }
                    (false, true, true) => {
                        fs::rename(
                            paths.stage_root.join(&journal.profile_id),
                            root.join(&journal.profile_id),
                        )
                        .map_err(|error| error.to_string())?;
                    }
                    (true, false, true) | (true, false, false) => {
                        return remove_profile_recovery_artifacts(paths)
                    }
                    (false, false, true) => {
                        fs::rename(
                            paths.backup_root.join(&journal.profile_id),
                            root.join(&journal.profile_id),
                        )
                        .map_err(|error| error.to_string())?;
                    }
                    _ => return Err("profile recovery artifacts are incomplete".into()),
                }
            }
            ProfileRecoveryAction::Rollback => match (final_present, backup_present) {
                (false, true) => {
                    fs::rename(
                        paths.backup_root.join(&journal.profile_id),
                        root.join(&journal.profile_id),
                    )
                    .map_err(|error| error.to_string())?;
                }
                (true, false) => return remove_profile_recovery_artifacts(paths),
                (true, true) => return Err("both final and rollback profiles exist".into()),
                (false, false) => {
                    return Err("rollback profile recovery artifacts are incomplete".into())
                }
            },
        }
    }
    Err("profile recovery did not converge".into())
}

fn recover_profile_transactions(root: &Path) -> Result<(), String> {
    let _guard = PROFILE_RECOVERY_LOCK
        .lock()
        .map_err(|_| "profile recovery lock is poisoned".to_string())?;
    let parent = root
        .parent()
        .ok_or("profile root has no parent directory")?;
    let prefix = profile_sibling_prefix(root, "recovery")?;
    let toggle_prefix = profile_sibling_prefix(root, "mod-toggle")?;
    let entries = match fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
    };
    let mut journals = Vec::new();
    let mut toggle_ids = Vec::new();
    let mut parent_entries = 0_usize;
    for entry in entries {
        parent_entries += 1;
        if parent_entries > MAX_PROFILE_RECOVERY_PARENT_ENTRIES {
            return Err("profile recovery directory contains too many entries".into());
        }
        let entry = entry.map_err(|error| error.to_string())?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let suffix = if let Some(suffix) = name.strip_prefix(&prefix) {
            suffix.to_owned()
        } else if let Some(id) = name.strip_prefix(&toggle_prefix) {
            validate_profile_id(id).map_err(|error| {
                format!(
                    "invalid mod toggle recovery marker {}: {error}",
                    entry.path().display()
                )
            })?;
            if toggle_ids.len() >= MAX_PROFILE_RECOVERY_JOURNALS {
                return Err("too many pending mod toggle recovery journals".into());
            }
            toggle_ids.push(id.to_string());
            continue;
        } else {
            continue;
        };
        if !valid_profile_transaction_suffix(&suffix) {
            return Err(format!(
                "invalid profile recovery marker {}",
                entry.path().display()
            ));
        }
        if journals.len() >= MAX_PROFILE_RECOVERY_JOURNALS {
            return Err("too many pending profile recovery journals".into());
        }
        let metadata = fs::symlink_metadata(entry.path()).map_err(|error| error.to_string())?;
        if is_reparse(&metadata) || !metadata.is_file() {
            return Err(format!(
                "profile recovery marker is not a regular file: {}",
                entry.path().display()
            ));
        }
        let bytes = read_bounded(&entry.path(), MAX_PROFILE_RECOVERY_JOURNAL_BYTES)?
            .ok_or("profile recovery marker disappeared")?;
        let journal: ProfileRecoveryJournal = serde_json::from_slice(&bytes)
            .map_err(|error| format!("invalid profile recovery marker: {error}"))?;
        if journal.version != 1 {
            return Err("unsupported profile recovery journal version".into());
        }
        validate_profile_id(&journal.profile_id)?;
        journals.push((name, profile_transaction_paths(root, &suffix)?, journal));
    }
    journals.sort_by(|left, right| left.0.cmp(&right.0));
    let mut ids = HashSet::new();
    let mut artifacts = HashSet::new();
    for (_, paths, journal) in &journals {
        if !ids.insert(journal.profile_id.to_ascii_lowercase())
            || !artifacts.insert(paths.stage_root.clone())
            || !artifacts.insert(paths.backup_root.clone())
        {
            return Err("ambiguous profile recovery journals were retained".into());
        }
    }
    for (_, paths, journal) in journals {
        if let Err(error) = recover_profile_transaction(root, &paths, &journal) {
            return Err(format!(
                "could not recover profile {}: {error}; recovery evidence was retained at {}",
                journal.profile_id,
                paths.journal.display()
            ));
        }
    }
    toggle_ids.sort_by_key(|id| id.to_ascii_lowercase());
    if toggle_ids
        .windows(2)
        .any(|ids| ids[0].eq_ignore_ascii_case(&ids[1]))
    {
        return Err("ambiguous mod toggle recovery journals were retained".into());
    }
    for id in toggle_ids {
        recover_mod_toggle_transaction(root, &id)?;
    }
    Ok(())
}

fn failed_profile_commit(
    backup: &Path,
    publish_error: &io::Error,
    rollback: Result<(), String>,
) -> String {
    match rollback {
        Ok(()) => format!("could not commit staged profile: {publish_error}"),
        Err(rollback_error) => format!(
            "could not commit staged profile ({publish_error}) or restore the old profile \
             ({rollback_error}); the intact backup and recovery journal were retained at {}",
            backup.display()
        ),
    }
}

fn profile_transaction<T>(
    root: &Path,
    id: &str,
    operation: impl FnOnce(&Path, &ProfileStore) -> Result<T, String>,
) -> Result<T, String> {
    let started = Instant::now();
    validate_profile_id(id)?;
    recover_profile_transactions(root)?;
    fs::create_dir_all(root).map_err(|error| error.to_string())?;
    let final_dir = root.join(id);
    let paths = allocate_profile_transaction_paths(root)?;
    fs::create_dir(&paths.stage_root).map_err(|error| error.to_string())?;
    let stage_dir = paths.stage_root.join(id);
    let old_exists = match fs::symlink_metadata(&final_dir) {
        Ok(metadata) if is_reparse(&metadata) || !metadata.is_dir() => {
            let _ = fs::remove_dir(&paths.stage_root);
            return Err("profile is not a regular directory".into());
        }
        Ok(_) => match stage_profile_tree(&final_dir, &stage_dir) {
            Ok(()) => true,
            Err(error) => {
                let _ = fs::remove_dir_all(&paths.stage_root);
                return Err(error);
            }
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            if let Err(error) = fs::create_dir(&stage_dir) {
                let _ = fs::remove_dir_all(&paths.stage_root);
                return Err(error.to_string());
            }
            false
        }
        Err(error) => {
            let _ = fs::remove_dir_all(&paths.stage_root);
            return Err(error.to_string());
        }
    };
    let stage_store = ProfileStore::new(&paths.stage_root);
    if old_exists {
        match stage_store.load(id) {
            Ok(Some(_)) => {}
            Ok(None) => {
                let _ = fs::remove_dir_all(&paths.stage_root);
                return Err("existing profile has no manifest".into());
            }
            Err(error) => {
                let _ = fs::remove_dir_all(&paths.stage_root);
                return Err(error.to_string());
            }
        }
    }
    let value = match operation(&paths.stage_root, &stage_store) {
        Ok(value) => value,
        Err(error) => {
            let _ = fs::remove_dir_all(&paths.stage_root);
            return Err(error);
        }
    };
    if let Err(error) = stage_store
        .load(id)
        .map_err(|error| error.to_string())
        .and_then(|record| record.ok_or("staged profile has no manifest".to_string()))
    {
        let _ = fs::remove_dir_all(&paths.stage_root);
        return Err(error);
    }
    let _recovery_guard = PROFILE_RECOVERY_LOCK
        .lock()
        .map_err(|_| "profile recovery lock is poisoned".to_string())?;
    let backup = paths.backup_root.join(id);
    if old_exists {
        fs::create_dir(&paths.backup_root).map_err(|error| {
            let _ = fs::remove_dir_all(&paths.stage_root);
            error.to_string()
        })?;
        if let Err(error) =
            write_profile_recovery_journal(&paths.journal, id, ProfileRecoveryAction::Publish)
        {
            let _ = fs::remove_dir_all(&paths.stage_root);
            let _ = fs::remove_dir(&paths.backup_root);
            return Err(error);
        }
        fs::rename(&final_dir, &backup).map_err(|error| {
            format!(
                "could not move the old profile into recovery storage ({error}); \
                 recovery evidence was retained at {}",
                paths.journal.display()
            )
        })?;
    }
    if let Err(error) = fs::rename(&stage_dir, &final_dir) {
        if !old_exists {
            let _ = fs::remove_dir_all(&paths.stage_root);
            return Err(format!("could not commit staged profile: {error}"));
        }
        if let Err(journal_error) =
            write_profile_recovery_journal(&paths.journal, id, ProfileRecoveryAction::Rollback)
        {
            return Err(format!(
                "could not commit staged profile ({error}) or record rollback intent \
                 ({journal_error}); recovery evidence was retained at {}",
                paths.journal.display()
            ));
        }
        let rollback = fs::rename(&backup, &final_dir).map_err(|rollback| rollback.to_string());
        if rollback.is_ok() {
            let _ = remove_profile_recovery_artifacts(&paths);
        }
        return Err(failed_profile_commit(&backup, &error, rollback));
    }
    if old_exists {
        remove_profile_recovery_artifacts(&paths)
            .map_err(|error| format!("profile committed but recovery cleanup failed: {error}"))?;
    } else {
        let _ = fs::remove_dir(&paths.stage_root);
    }
    log_perf("profile_transaction", started, 0, 0);
    Ok(value)
}

fn delete_profile_transaction(root: &Path, id: &str) -> Result<(), String> {
    validate_profile_id(id)?;
    recover_profile_transactions(root)?;
    let final_dir = root.join(id);
    let metadata = match fs::symlink_metadata(&final_dir) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
    };
    if is_reparse(&metadata) || !metadata.is_dir() {
        return Err("profile is not a regular directory".into());
    }
    let trash_root = unique_sibling(root, "deleted")?;
    fs::create_dir(&trash_root).map_err(|error| error.to_string())?;
    let trash = trash_root.join(id);
    if let Err(error) = fs::rename(&final_dir, &trash) {
        let _ = fs::remove_dir(&trash_root);
        return Err(error.to_string());
    }
    let _ = fs::remove_dir_all(&trash_root);
    Ok(())
}

fn legacy_cached_catalog_text() -> Result<Option<String>, String> {
    let Some(bytes) = read_bounded(&settings::catalog_cache_path(), MAX_CATALOG_BYTES)? else {
        return Ok(None);
    };
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|error| format!("invalid catalog UTF-8: {error}"))
}

fn legacy_catalog() -> Result<Catalog, String> {
    match legacy_cached_catalog_text()? {
        Some(text) => parse(&text).map_err(|error| format!("invalid cached catalog: {error}")),
        None => Ok(bundled_catalog()),
    }
}

fn validate_persisted_catalog(catalog: Catalog) -> Result<Catalog, String> {
    let encoded = serde_json::to_string(&catalog).map_err(|error| error.to_string())?;
    parse(&encoded).map_err(|error| format!("invalid persisted hosted catalog: {error}"))
}

fn catalog() -> Result<Catalog, String> {
    let persisted = read_bounded(&settings::user_catalog_path(), MAX_CATALOG_ENVELOPE_BYTES)?
        .and_then(|bytes| serde_json::from_slice::<CatalogEnvelope>(&bytes).ok())
        .and_then(|envelope| envelope.hosted_catalog);
    let mut active = match persisted {
        Some(hosted) => validate_persisted_catalog(hosted)?,
        None => legacy_catalog()?,
    };
    apply_bundled_install_policy(&mut active);
    Ok(active)
}

/// The catalog compiled into this build (always current with the app). Used for
/// the loader source so a stale on-disk mod cache can't break BepInEx install.
fn bundled_catalog() -> Catalog {
    parse(BUNDLED_CATALOG).expect("bundled catalog parses")
}

/// Dependency graphs and asset selection are executable install policy. Bundled
/// entries therefore override stale hosted/cache policy while descriptive data stays live.
fn apply_bundled_install_policy(active: &mut Catalog) {
    let bundled = bundled_catalog();
    for entry in &mut active.mods {
        if let Some(authoritative) = bundled.get(&entry.id) {
            entry.asset_rules = authoritative.asset_rules.clone();
            entry.dependencies = authoritative.dependencies.clone();
            entry.provides = authoritative.provides.clone();
            entry.dependency_versions = authoritative.dependency_versions.clone();
            entry.recommended_dependencies = authoritative.recommended_dependencies.clone();
        }
    }
}

fn apply_bundled_display_policy(list: &mut [CatalogListItem]) {
    let bundled = bundled_catalog();
    for item in list {
        if let Some(authoritative) = bundled.get(&item.id) {
            item.dependencies = authoritative.dependencies.clone();
            item.provides = authoritative.provides.clone();
            item.dependency_versions = authoritative.dependency_versions.clone();
            item.recommended_dependencies = authoritative.recommended_dependencies.clone();
            item.included = catalog_item(authoritative.clone()).included;
        }
    }
}

fn catalog_display_rank(item: &CatalogListItem) -> usize {
    PRIORITY_CATALOG_IDS
        .iter()
        .position(|id| id.eq_ignore_ascii_case(&item.id))
        .unwrap_or_else(|| {
            if item.tags.contains(&ModTag::Library)
                || TOU_BUNDLED_DEPENDENCY_IDS
                    .iter()
                    .any(|id| id.eq_ignore_ascii_case(&item.id))
            {
                usize::MAX
            } else {
                PRIORITY_CATALOG_IDS.len()
            }
        })
}

fn apply_default_catalog_order(list: &mut [CatalogListItem]) {
    list.sort_by_key(catalog_display_rank);
}

fn recovered_profile_store(root: &Path) -> Result<ProfileStore, String> {
    recover_profile_transactions(root)?;
    Ok(ProfileStore::new(root))
}

fn store() -> Result<ProfileStore, String> {
    recovered_profile_store(&settings::profiles_root())
}

fn http() -> Result<UreqHttp, String> {
    let token = settings::github_token().map_err(|error| error.to_string())?;
    let exposed = token
        .as_ref()
        .map(|secret| secret.expose_secret().to_owned());
    let fingerprint = sha256_hex(exposed.as_deref().unwrap_or_default().as_bytes());
    let mut shared = HTTP_CLIENT
        .lock()
        .map_err(|_| "HTTP client lock is poisoned".to_string())?;
    if let Some((cached_fingerprint, client)) = shared.as_ref() {
        if cached_fingerprint == &fingerprint {
            return Ok(client.clone());
        }
    }
    let client = UreqHttp::new(exposed);
    *shared = Some((fingerprint, client.clone()));
    Ok(client)
}

fn fetch_releases_bounded(
    client: &UreqHttp,
    repo: &str,
    limit: u32,
) -> Result<Vec<Release>, String> {
    let started = Instant::now();
    let tags =
        resolver::fetch_release_tags(client, repo, limit).map_err(|error| error.to_string())?;
    let mut releases = Vec::with_capacity(tags.len());
    for chunk in tags.chunks(4) {
        let resolved = std::thread::scope(|scope| {
            chunk
                .iter()
                .map(|tag| {
                    let client = client.clone();
                    let repo = repo.to_string();
                    let tag = tag.clone();
                    scope.spawn(move || {
                        resolver::fetch_release_by_tag(&client, &repo, &tag)
                            .map_err(|error| error.to_string())
                    })
                })
                .map(|handle| {
                    handle
                        .join()
                        .map_err(|_| "release metadata worker panicked".to_string())?
                })
                .collect::<Result<Vec<_>, String>>()
        })?;
        releases.extend(resolved);
    }
    log_perf("release_metadata", started, releases.len(), 0);
    Ok(releases)
}

fn download_resolved(
    client: &dyn Http,
    resolved: &ResolvedDownload,
) -> Result<Vec<u8>, perfect_sync_core::resolver::ResolveError> {
    use perfect_sync_core::resolver::ResolveError;
    let started = Instant::now();
    let Some(cache_dir) = settings::cache_dir_if_initialized() else {
        let result = download_resolved_uncached(client, resolved);
        if result.is_ok() {
            log_perf("asset_download", started, 1, resolved.size.bytes());
        }
        return result;
    };

    let cache_root = cache_dir.join("assets");
    let blobs = cache_root.join("sha256");
    let requests = cache_root.join("requests");
    let mut identity = Vec::with_capacity(resolved.url.len() + 32);
    identity.extend_from_slice(resolved.url.as_bytes());
    identity.push(0);
    identity.extend_from_slice(&resolved.size.bytes().to_le_bytes());
    let request_key = sha256_hex(&identity);
    let _guard = lock_asset_cache(&request_key).map_err(ResolveError::Http)?;
    let request_path = requests.join(&request_key);
    let expected_digest = resolved.size.sha256().map(|digest| sha256_hex(&digest));
    let indexed_digest = read_bounded(&request_path, 64)
        .ok()
        .flatten()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .filter(|digest| digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit()));
    if let Some(digest) = expected_digest.as_ref().or(indexed_digest.as_ref()) {
        let path = blobs.join(digest);
        if let Ok(metadata) = fs::symlink_metadata(&path) {
            if metadata.is_file()
                && !is_reparse(&metadata)
                && metadata.len() == resolved.size.bytes()
            {
                let bytes =
                    fs::read(&path).map_err(|error| ResolveError::Http(error.to_string()))?;
                if sha256_hex(&bytes).eq_ignore_ascii_case(digest) {
                    log_perf("asset_cache_hit", started, 1, bytes.len() as u64);
                    return Ok(bytes);
                }
            }
            let _ = fs::remove_file(path);
        }
    }

    fs::create_dir_all(&blobs).map_err(|error| ResolveError::Http(error.to_string()))?;
    fs::create_dir_all(&requests).map_err(|error| ResolveError::Http(error.to_string()))?;
    let temporary =
        unique_sibling(&blobs.join(&request_key), "download").map_err(ResolveError::Http)?;
    let download_result = (|| {
        let mut output = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .map_err(|error| ResolveError::Http(error.to_string()))?;
        download_resolved_to_writer(client, resolved, &mut output, &mut |_, _| {})?;
        output
            .sync_all()
            .map_err(|error| ResolveError::Http(error.to_string()))?;
        drop(output);
        let bytes = fs::read(&temporary).map_err(|error| ResolveError::Http(error.to_string()))?;
        let digest = sha256_hex(&bytes);
        let blob = blobs.join(&digest);
        if blob.is_file() {
            fs::remove_file(&temporary).map_err(|error| ResolveError::Http(error.to_string()))?;
        } else {
            fs::rename(&temporary, &blob).map_err(|error| ResolveError::Http(error.to_string()))?;
        }
        atomic_write(&request_path, digest.as_bytes()).map_err(ResolveError::Http)?;
        Ok(bytes)
    })();
    if download_result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    if let Ok(bytes) = &download_result {
        log_perf("asset_download", started, 1, bytes.len() as u64);
    }
    download_result
}

fn default_rules() -> AssetRules {
    AssetRules {
        per_arch: HashMap::<String, AssetArchRule>::new(),
        dll_name: None,
        bundles_loader: false,
    }
}

fn selected_profile_instance<'a>(
    saved: &'a Settings,
    instance_id: Option<&str>,
) -> Result<&'a settings::GameInstance, String> {
    let instance_id = instance_id.ok_or("profile has no saved game instance")?;
    saved
        .game_instances
        .iter()
        .find(|instance| instance.id == instance_id)
        .ok_or_else(|| format!("profile refers to unknown game instance {instance_id}"))
}

fn saved_game_arch(instance_id: Option<&str>) -> Result<String, String> {
    let saved = settings::load().map_err(|error| error.to_string())?;
    let instance = selected_profile_instance(&saved, instance_id)?;
    let game_dir = canonical_game_path(Path::new(&instance.path))?;
    game::exe_arch(&game_dir.join(process::GAME_EXE))
        .map(arch_str)
        .ok_or("Among Us executable architecture is unsupported".to_string())
}

fn profile_arch(profile_id: &str) -> Result<String, String> {
    validate_profile_id(profile_id)?;
    let record = store()?
        .load(profile_id)
        .map_err(|error| error.to_string())?
        .ok_or("profile not found")?;
    saved_game_arch(record.game_instance_id.as_deref())
}

fn profile_store_runtime(profile_id: &str) -> Result<(Store, Runtime), String> {
    validate_profile_id(profile_id)?;
    let record = store()?
        .load(profile_id)
        .map_err(|error| error.to_string())?
        .ok_or("profile not found")?;
    let saved = settings::load().map_err(|error| error.to_string())?;
    let instance = selected_profile_instance(&saved, record.game_instance_id.as_deref())?;
    Ok((instance.store, instance.runtime))
}

fn is_tou_mira(identity: &str) -> bool {
    identity.eq_ignore_ascii_case(TOU_MIRA_ID)
}

fn tou_package_key(version: &str, asset_name: &str) -> String {
    let mut identity = Vec::with_capacity(version.len() + asset_name.len() + 1);
    identity.extend_from_slice(version.as_bytes());
    identity.push(0);
    identity.extend_from_slice(asset_name.as_bytes());
    sha256_hex(&identity)
}

fn tou_package_cache_path(version: &str, asset_name: &str) -> PathBuf {
    settings::cache_dir()
        .join("tou-mira")
        .join(format!("{}.zip", tou_package_key(version, asset_name)))
}

fn cache_tou_package(version: &str, asset_name: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    if bytes.is_empty() || bytes.len() as u64 > MAX_TOU_PACKAGE_CACHE_BYTES {
        return Err("Town of Us package has an invalid download size".into());
    }
    let path = tou_package_cache_path(version, asset_name);
    if let Some(existing) = read_bounded(&path, MAX_TOU_PACKAGE_CACHE_BYTES)? {
        if existing == bytes {
            return Ok(path);
        }
    }
    atomic_write(&path, bytes)?;
    Ok(path)
}

fn tou_asset_fragment(arch: &str, store: Store, runtime: Runtime) -> Result<&'static str, String> {
    match arch {
        "x64" => Ok("x64-epic-msstore.zip"),
        "x86" if runtime != Runtime::Native => Ok("x86-macos-linux.zip"),
        "x86" if matches!(store, Store::Steam | Store::Itch | Store::Manual) => {
            Ok("x86-steam-itch.zip")
        }
        "x86" => Err("Town of Us has no x86 Epic/MS Store package for native Windows".into()),
        _ => Err("Among Us executable architecture is unsupported".into()),
    }
}

fn pick_profile_asset<'a>(
    release: &'a Release,
    repo: &str,
    rules: &AssetRules,
    arch: &str,
    store: Store,
    runtime: Runtime,
) -> Result<Option<&'a resolver::Asset>, String> {
    if !is_tou_mira(repo) {
        return Ok(resolver::pick_asset(release, rules, arch));
    }
    let fragment = tou_asset_fragment(arch, store, runtime)?;
    let mut matches = release.assets.iter().filter(|asset| {
        let name = asset.name.to_ascii_lowercase();
        name.ends_with(".zip") && name.contains(fragment)
    });
    let selected = matches.next();
    if selected.is_some() && matches.next().is_some() {
        return Err(format!(
            "Town of Us release {} has multiple {fragment} packages",
            release.tag
        ));
    }
    Ok(selected)
}

fn resolve_profile_tag(
    http: &dyn Http,
    repo: &str,
    tag: &str,
    rules: &AssetRules,
    arch: &str,
    store: Store,
    runtime: Runtime,
) -> Result<ResolvedDownload, String> {
    if !is_tou_mira(repo) {
        return resolver::resolve_tag(http, repo, tag, rules, arch)
            .map_err(|error| error.to_string());
    }
    let release =
        resolver::fetch_release_by_tag(http, repo, tag).map_err(|error| error.to_string())?;
    let asset = pick_profile_asset(&release, repo, rules, arch, store, runtime)?
        .ok_or_else(|| format!("Town of Us {tag} has no compatible full package"))?;
    resolver::resolved_asset(http, &release, asset).map_err(|error| error.to_string())
}

fn resolve_profile_latest(
    http: &dyn Http,
    repo: &str,
    rules: &AssetRules,
    arch: &str,
    store: Store,
    runtime: Runtime,
) -> Result<ResolvedDownload, String> {
    if !is_tou_mira(repo) {
        return resolver::resolve_latest(http, repo, rules, arch)
            .map_err(|error| error.to_string());
    }
    let release = resolver::fetch_latest_release(http, repo).map_err(|error| error.to_string())?;
    let asset = pick_profile_asset(&release, repo, rules, arch, store, runtime)?
        .ok_or_else(|| format!("Town of Us {} has no compatible full package", release.tag))?;
    resolver::resolved_asset(http, &release, asset).map_err(|error| error.to_string())
}

fn latest_profile_version(
    http: &dyn Http,
    repo: &str,
    rules: &AssetRules,
    arch: &str,
    store: Store,
    runtime: Runtime,
) -> Result<String, String> {
    let release =
        resolver::fetch_latest_release_fresh(http, repo).map_err(|error| error.to_string())?;
    if pick_profile_asset(&release, repo, rules, arch, store, runtime)?.is_none() {
        return Err(if is_tou_mira(repo) {
            format!("Town of Us {} has no compatible full package", release.tag)
        } else {
            format!("{repo} release {} has no compatible package", release.tag)
        });
    }
    Ok(release.tag)
}

fn arch_str(a: Arch) -> String {
    match a {
        Arch::X86 => "x86".to_string(),
        Arch::X64 => "x64".to_string(),
    }
}

fn same_path(a: &Path, b: &Path) -> bool {
    matches!(
        (fs::canonicalize(a), fs::canonicalize(b)),
        (Ok(a), Ok(b)) if a == b
    )
}

fn validate_game_dir(game_dir: &Path) -> Result<(), String> {
    if !game_dir.is_dir() {
        return Err(format!("game folder not found: {}", game_dir.display()));
    }
    let executable = game_dir.join(process::GAME_EXE);
    let metadata = fs::symlink_metadata(&executable).map_err(|_| {
        format!(
            "This is not the Among Us folder: {} is missing",
            executable.display()
        )
    })?;
    if is_reparse(&metadata) || !metadata.is_file() {
        return Err("Among Us executable is not a regular file".into());
    }
    game::exe_arch(&executable)
        .ok_or_else(|| "Among Us executable architecture is unsupported".to_string())
        .map(|_| ())
}

fn copy_game_tree(
    source: &Path,
    destination: &Path,
    files: &mut usize,
    bytes: &mut u64,
) -> Result<(), String> {
    let source_metadata = fs::symlink_metadata(source).map_err(|error| error.to_string())?;
    if is_reparse(&source_metadata) || !source_metadata.is_dir() {
        return Err(format!(
            "Backup copies cannot follow links or reparse points: {}",
            source.display()
        ));
    }
    fs::create_dir(destination).map_err(|error| error.to_string())?;
    for entry in fs::read_dir(source).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let name = entry.file_name();
        if name.to_string_lossy().starts_with(".perfectsync-") {
            continue;
        }
        let from = entry.path();
        let to = destination.join(name);
        let metadata = fs::symlink_metadata(&from).map_err(|error| error.to_string())?;
        if is_reparse(&metadata) {
            return Err(format!(
                "Backup copies cannot follow links or reparse points: {}",
                from.display()
            ));
        }
        if metadata.is_dir() {
            copy_game_tree(&from, &to, files, bytes)?;
            continue;
        }
        if !metadata.is_file() {
            return Err(format!("Unsupported game file type: {}", from.display()));
        }
        *files = files
            .checked_add(1)
            .ok_or("Backup copy file count overflow")?;
        *bytes = bytes
            .checked_add(metadata.len())
            .ok_or("Backup copy size overflow")?;
        if *files > MAX_RECURSIVE_COPY_FILES || *bytes > MAX_RECURSIVE_COPY_BYTES {
            return Err("The backup exceeds its copy safety limit.".into());
        }
        let mut input = File::open(&from).map_err(|error| error.to_string())?;
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&to)
            .map_err(|error| error.to_string())?;
        io::copy(&mut input, &mut output).map_err(|error| error.to_string())?;
        output.sync_all().map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn canonical_game_path(game_dir: &Path) -> Result<PathBuf, String> {
    if !game_dir.is_dir() {
        return Err(format!("game folder not found: {}", game_dir.display()));
    }
    let canonical = fs::canonicalize(game_dir).map_err(|error| error.to_string())?;
    let executable = canonical.join(process::GAME_EXE);
    let metadata = fs::symlink_metadata(&executable).map_err(|_| {
        format!(
            "This is not the Among Us folder: {} is missing",
            executable.display()
        )
    })?;
    if is_reparse(&metadata) || !metadata.is_file() {
        return Err("Among Us executable is not a regular file".into());
    }
    game::exe_arch(&executable).ok_or("Among Us executable architecture is unsupported")?;
    Ok(canonical)
}
#[cfg(windows)]
fn game_executable_identity(game_dir: &Path) -> Option<String> {
    use std::ffi::c_void;
    use std::os::windows::io::AsRawHandle;

    #[repr(C)]
    struct FileTime {
        low: u32,
        high: u32,
    }

    #[repr(C)]
    struct ByHandleFileInformation {
        file_attributes: u32,
        creation_time: FileTime,
        last_access_time: FileTime,
        last_write_time: FileTime,
        volume_serial_number: u32,
        file_size_high: u32,
        file_size_low: u32,
        number_of_links: u32,
        file_index_high: u32,
        file_index_low: u32,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetFileInformationByHandle(
            file: *mut c_void,
            information: *mut ByHandleFileInformation,
        ) -> i32;
    }

    let executable = File::open(game_dir.join(process::GAME_EXE)).ok()?;
    let mut information: ByHandleFileInformation = unsafe { std::mem::zeroed() };
    let succeeded =
        unsafe { GetFileInformationByHandle(executable.as_raw_handle(), &mut information) };
    if succeeded == 0 {
        return None;
    }
    let file_index =
        ((information.file_index_high as u64) << 32) | information.file_index_low as u64;
    Some(format!(
        "{:08x}:{file_index:016x}",
        information.volume_serial_number
    ))
}

#[cfg(unix)]
fn game_executable_identity(game_dir: &Path) -> Option<String> {
    use std::os::unix::fs::MetadataExt;

    let metadata = fs::metadata(game_dir.join(process::GAME_EXE)).ok()?;
    Some(format!("{:016x}:{:016x}", metadata.dev(), metadata.ino()))
}
fn refresh_game_instance(
    instance: &mut settings::GameInstance,
    canonical: &Path,
) -> Result<bool, String> {
    let previous = instance.clone();
    let path_changed = !same_path(Path::new(&instance.path), canonical);
    instance.path = canonical.to_string_lossy().into_owned();
    instance.executable_identity = game_executable_identity(canonical);
    instance.arch = game::exe_arch(&canonical.join(process::GAME_EXE))
        .ok_or("Among Us executable architecture is unsupported")?;
    instance.runtime = compat::resolve_with_hint(canonical, Some(instance.runtime)).runtime;
    if path_changed {
        instance.source_fingerprint = None;
        instance.source_file_count = None;
        instance.source_byte_count = None;
    }
    if instance.source_fingerprint.is_none() {
        instance.build = game::detect_build(canonical);
    }
    instance.writable = game::is_writable_game_dir(canonical);
    let detected_store = game::store_for_path(canonical, Store::Manual);
    if detected_store != Store::Manual {
        instance.store = detected_store;
    }
    Ok(*instance != previous)
}

fn repair_moved_game_instances(saved: &mut Settings) -> Result<(bool, Vec<String>), String> {
    let mut changed = false;
    let mut repaired = Vec::new();
    for instance in &mut saved.game_instances {
        if let Ok(canonical) = canonical_game_path(Path::new(&instance.path)) {
            changed |= refresh_game_instance(instance, &canonical)?;
            continue;
        }

        let original = PathBuf::from(&instance.path);
        if original.is_dir() {
            continue;
        }
        let Some(parent) = original.parent().filter(|parent| parent.is_dir()) else {
            continue;
        };
        let expected_identity = instance.executable_identity.as_deref();
        let mut candidates = Vec::new();
        for entry in fs::read_dir(parent)
            .map_err(|error| format!("Could not inspect the old game folder's parent: {error}"))?
            .take(MAX_PROFILE_RECOVERY_PARENT_ENTRIES + 1)
        {
            let entry = entry.map_err(|error| error.to_string())?;
            let metadata = fs::symlink_metadata(entry.path()).map_err(|error| error.to_string())?;
            if is_reparse(&metadata) || !metadata.is_dir() {
                continue;
            }
            let Ok(candidate) = canonical_game_path(&entry.path()) else {
                continue;
            };
            if game::exe_arch(&candidate.join(process::GAME_EXE)) != Some(instance.arch) {
                continue;
            }
            if instance
                .build
                .as_deref()
                .is_some_and(|expected| game::detect_build(&candidate).as_deref() != Some(expected))
            {
                continue;
            }
            let candidate_identity = game_executable_identity(&candidate);
            if expected_identity.is_some() && candidate_identity.as_deref() != expected_identity {
                continue;
            }
            candidates.push((candidate, candidate_identity));
        }
        if candidates.len() != 1 {
            continue;
        }
        let (candidate, _) = candidates.pop().unwrap();
        let rebound = if instance.source_fingerprint.is_some() {
            managed_instance::rebind_source_record(instance, &candidate)?
        } else {
            None
        };
        refresh_game_instance(instance, &candidate)?;
        if let Some(source) = rebound {
            instance.source_fingerprint = Some(source.record.fingerprint);
            instance.source_file_count = Some(source.record.file_count);
            instance.source_byte_count = Some(source.record.byte_count);
            instance.build = source.record.observed_build;
        }
        repaired.push(instance.name.clone());
        changed = true;
    }
    Ok((changed, repaired))
}

fn validate_game_target(game_path: &str, profile_id: Option<&str>) -> Result<PathBuf, String> {
    let canonical = canonical_game_path(Path::new(game_path))?;
    let saved = settings::load().map_err(|error| error.to_string())?;
    if let Some(profile_id) = profile_id {
        validate_profile_id(profile_id)?;
        let record = store()?
            .load(profile_id)
            .map_err(|error| error.to_string())?
            .ok_or("profile not found")?;
        let instance = selected_profile_instance(&saved, record.game_instance_id.as_deref())?;
        if !same_path(Path::new(&instance.path), &canonical) {
            return Err("game folder does not match the profile's saved instance".into());
        }
        return Ok(canonical);
    }
    if saved
        .game_instances
        .iter()
        .any(|instance| same_path(Path::new(&instance.path), &canonical))
    {
        return Ok(canonical);
    }
    if INSPECTED_GAMES
        .lock()
        .map_err(|_| "inspected game lock is poisoned")?
        .contains(&canonical)
    {
        return Ok(canonical);
    }
    Err("game folder must be saved or explicitly inspected before use".into())
}

fn configure_runtime_override(ctx: &compat::RuntimeContext) -> Result<(), String> {
    if ctx.runtime == Runtime::Native {
        return Ok(());
    }
    let prefix = ctx.prefix.as_ref().ok_or_else(|| {
        "No Wine prefix could be derived for this game folder. Select the real folder inside the Wine/CrossOver/Whisky/Bottles prefix.".to_string()
    })?;
    compat::register_winhttp_override(prefix).map_err(|e| {
        if ctx.runtime == Runtime::Proton && e.kind() == std::io::ErrorKind::NotFound {
            format!(
                "Steam has not created the Among Us Proton prefix yet. Launch Among Us once without mods, close it, then retry. Folder setup is already safe to run. ({e})"
            )
        } else {
            format!(
                "Could not configure BepInEx's winhttp override in {}: {e}",
                prefix.display()
            )
        }
    })
}

/// Friendly guidance when a non-native launcher cannot be started.
fn launch_err_msg(ctx: &compat::RuntimeContext, e: &std::io::Error) -> String {
    match ctx.runtime {
        Runtime::Proton => format!(
            "Couldn't start the selected Proton tool ({e}). The isolated workspace is still ready; verify Proton in Steam, then retry."
        ),
        Runtime::Wine => format!(
            "Couldn't run Wine ({e}). The isolated workspace is still ready; verify the configured Wine runtime, then retry."
        ),
        Runtime::Crossover => format!(
            "Couldn't run CrossOver's Wine launcher ({e}). The isolated workspace is still ready; verify the selected bottle and CrossOver installation, then retry."
        ),
        Runtime::Whisky => format!(
            "Couldn't run Whisky's Wine ({e}). The isolated workspace is still ready; verify the selected bottle, then retry."
        ),
        Runtime::Bottles => format!(
            "Couldn't run bottles-cli ({e}). The isolated workspace is still ready; verify the selected bottle, then retry."
        ),
        Runtime::Native => format!("Failed to launch the game: {e}"),
    }
}
fn ensure_or_refresh_source(
    instance: &settings::GameInstance,
    required_build: Option<&str>,
) -> Result<(), String> {
    if let Ok(source) = managed_instance::source_for_rebuild(instance, required_build) {
        if managed_instance::ensure_exact_source_available(&source).is_ok() {
            return Ok(());
        }
    }

    let canonical = canonical_game_path(Path::new(&instance.path))?;
    if !same_path(Path::new(&instance.path), &canonical) {
        return Err(
            "The saved Among Us source path changed. Open Settings to select it again.".into(),
        );
    }
    let arch = game::exe_arch(&canonical.join(process::GAME_EXE))
        .ok_or("Among Us executable architecture is unsupported")?;
    if arch != instance.arch {
        return Err(
            "The original Among Us source architecture changed. Re-resolve this profile before installing mods."
                .into(),
        );
    }
    let store = game::store_for_path(&canonical, instance.store);
    if store != instance.store {
        return Err(
            "The original Among Us source storefront changed. Re-resolve this profile before installing mods."
                .into(),
        );
    }
    let build = game::detect_build(&canonical);
    if required_build.is_some_and(|required| build.as_deref() != Some(required)) {
        return Err(format!(
            "The original Among Us source is now build {}, but this profile requires build {}. Its existing direct instance remains playable.",
            build.as_deref().unwrap_or("unknown"),
            required_build.unwrap_or("unknown"),
        ));
    }

    let mut refreshed = instance.clone();
    refreshed.path = canonical.to_string_lossy().into_owned();
    refreshed.executable_identity = game_executable_identity(&canonical);
    refreshed.source_fingerprint = None;
    refreshed.source_file_count = None;
    refreshed.source_byte_count = None;
    refreshed.runtime = compat::resolve_with_hint(&canonical, Some(instance.runtime)).runtime;
    refreshed.build = build;
    refreshed.writable = game::is_writable_game_dir(&canonical);
    let source = managed_instance::record_source(&refreshed)?;
    refreshed.source_fingerprint = Some(source.record.fingerprint);
    refreshed.source_file_count = Some(source.record.file_count);
    refreshed.source_byte_count = Some(source.record.byte_count);
    if !settings::update_game_instance_source(instance, &refreshed)
        .map_err(|error| error.to_string())?
    {
        let current = settings::load()
            .map_err(|error| error.to_string())?
            .game_instances
            .into_iter()
            .find(|current| current.id == instance.id)
            .ok_or("The selected Among Us source was removed while it was being refreshed.")?;
        let source = managed_instance::source_for_rebuild(&current, required_build)?;
        return managed_instance::ensure_exact_source_available(&source);
    }
    log::info!(
        "refreshed changed original source record automatically for {}",
        instance.name
    );
    Ok(())
}

fn require_profile_source_for_install(profile_id: &str) -> Result<(), String> {
    managed_instance::migrate_direct_source_storage()?;
    let profiles = recovered_profile_store(&settings::profiles_root())?;
    let profile = profiles
        .load(profile_id)
        .map_err(|error| error.to_string())?
        .ok_or("profile not found")?;
    let saved = settings::load().map_err(|error| error.to_string())?;
    let instance = selected_profile_instance(&saved, profile.game_instance_id.as_deref())?;
    ensure_or_refresh_source(instance, profile.game_build.as_deref())
}

fn require_instance_source_for_install(game_instance_id: Option<&str>) -> Result<(), String> {
    managed_instance::migrate_direct_source_storage()?;
    let saved = settings::load().map_err(|error| error.to_string())?;
    let instance = selected_profile_instance(&saved, game_instance_id)?;
    ensure_or_refresh_source(instance, instance.build.as_deref())
}

/// Download a verified release asset and install its declared plugin DLL.
fn install_resolved(
    profiles_root: &Path,
    profile_id: &str,
    http: &dyn Http,
    resolved: &ResolvedDownload,
    expected_dll: Option<&str>,
) -> Result<String, String> {
    let asset = resolved.asset_name.to_ascii_lowercase();
    let bytes = download_resolved(http, resolved).map_err(|error| error.to_string())?;
    if asset.ends_with(".dll") {
        profile::install_plugin_bytes(profiles_root, profile_id, &resolved.asset_name, &bytes)
            .map_err(|error| error.to_string())?;
        return Ok(resolved.asset_name.clone());
    }
    if asset.ends_with(".zip") {
        let dll_name = expected_dll.ok_or(
            "Catalog ZIP installs require an exact declared DLL name. Pick a DLL file manually.",
        )?;
        profile::install_plugin_zip_bytes(profiles_root, profile_id, dll_name, &bytes)
            .map_err(|error| error.to_string())?;
        return Ok(dll_name.to_string());
    }
    Err("Only .dll files and catalog-declared .zip packages can be installed.".into())
}

fn pinned_loader(arch: &str) -> Result<(String, String), String> {
    let url = match arch {
        "x86" => PINNED_LOADER_X86_URL,
        "x64" => PINNED_LOADER_X64_URL,
        _ => return Err("Among Us executable architecture is unsupported".into()),
    };
    Ok((PINNED_LOADER_VERSION.to_string(), url.to_string()))
}

/// Resolve the newest BepInEx loader for the explicit Advanced action.
/// The normal setup and reinstall paths use the pinned build instead.
fn resolve_loader(http: &dyn Http, arch: &str) -> Result<(String, String), String> {
    let loader = bundled_catalog()
        .loader
        .ok_or("catalog has no loader entry")?;
    if let Some(builds) = &loader.builds_url {
        if let Ok(html) = http.get_text(builds) {
            if let Some(pair) = loader::parse_latest_build(&html, arch) {
                return Ok(pair);
            }
        }
    }
    if let Some(api) = &loader.thunderstore_api {
        if let Ok(text) = http.get_text(api) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                if let (Some(ver), Some(url)) = (
                    v["latest"]["version_number"].as_str(),
                    v["latest"]["download_url"].as_str(),
                ) {
                    return Ok((format!("au-{ver}"), url.to_string()));
                }
            }
        }
    }
    if let Some(u) = &loader.pack_url {
        if !u.is_empty() {
            return Ok(("pinned".to_string(), u.clone()));
        }
    }
    Err("could not resolve a BepInEx loader source (check your internet)".to_string())
}

fn download_doorstop_fix(http: &dyn Http) -> Result<Vec<u8>, String> {
    let bytes = http
        .get_bytes(DOORSTOP_FIX_URL)
        .map_err(|error| format!("could not download the BepInEx compatibility fix: {error}"))?;
    if bytes.len() != DOORSTOP_FIX_SIZE {
        return Err(format!(
            "BepInEx compatibility fix size mismatch: expected {DOORSTOP_FIX_SIZE} bytes, received {}",
            bytes.len()
        ));
    }
    if sha256_hex(&bytes) != DOORSTOP_FIX_SHA256 {
        return Err("BepInEx compatibility fix failed SHA-256 verification".into());
    }
    Ok(bytes)
}

fn doorstop_fix_is_current(game_dir: &Path, arch: &str) -> bool {
    loader::has_doorstop_patch(game_dir, DOORSTOP_FIX_VERSION, arch)
}

// ---------- settings + detection ----------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GameInstallView {
    #[serde(flatten)]
    install: game::GameInstall,
    source_clean: bool,
    source_mod_artifacts: Vec<String>,
}

fn game_install_view(install: game::GameInstall) -> Result<GameInstallView, String> {
    let artifacts = managed_instance::source_mod_artifacts(&install.path)?;
    Ok(GameInstallView {
        install,
        source_clean: artifacts.is_empty(),
        source_mod_artifacts: artifacts,
    })
}
fn fresh_game_install_view(install: game::GameInstall) -> Result<Option<GameInstallView>, String> {
    game_install_view(install).map(|view| view.source_clean.then_some(view))
}

#[tauri::command]
pub async fn detect_games() -> Result<Vec<GameInstallView>, String> {
    blocking(|| {
        game::locate_all()
            .into_iter()
            .filter_map(|install| fresh_game_install_view(install).transpose())
            .collect()
    })
    .await
}

#[tauri::command]
pub async fn inspect_game(game_path: String) -> Result<GameInstallView, String> {
    blocking(move || {
        let _guard = lock_mutations()?;
        let canonical = canonical_game_path(Path::new(&game_path))?;
        let store = game::store_for_path(&canonical, Store::Manual);
        let arch = game::exe_arch(&canonical.join(process::GAME_EXE))
            .ok_or("Among Us executable architecture is unsupported")?;
        let runtime = compat::resolve(&canonical).runtime;
        INSPECTED_GAMES
            .lock()
            .map_err(|_| "inspected game lock is poisoned")?
            .insert(canonical.clone());
        game_install_view(game::GameInstall {
            path: canonical.clone(),
            store,
            arch,
            runtime,
            build: game::detect_build(&canonical),
            writable: game::is_writable_game_dir(&canonical),
        })
    })
    .await
}

#[tauri::command]
pub async fn get_settings() -> Result<SettingsView, String> {
    blocking(|| {
        let _guard = lock_mutations()?;
        managed_instance::migrate_direct_source_storage()?;
        let mut view = settings::view().map_err(|error| error.to_string())?;
        let mut changed = false;
        for instance in &mut view.settings.game_instances {
            if instance.source_fingerprint.is_none() {
                if let Some(source) = managed_instance::saved_source(instance)? {
                    if instance.build.is_none() && source.record.observed_build.is_some() {
                        instance.build = source.record.observed_build.clone();
                    }
                    instance.source_fingerprint = Some(source.record.fingerprint);
                    instance.source_file_count = Some(source.record.file_count);
                    instance.source_byte_count = Some(source.record.byte_count);
                    changed = true;
                }
            }
        }
        let (metadata_changed, repaired) = repair_moved_game_instances(&mut view.settings)?;
        changed |= metadata_changed;
        if changed {
            settings::save(&view.settings).map_err(|error| error.to_string())?;
        }
        if !repaired.is_empty() {
            log::info!(
                "recovered renamed Among Us folders for {}",
                repaired.join(", ")
            );
        }
        Ok(view)
    })
    .await
}

#[tauri::command]
pub async fn select_active_profile(profile_id: String) -> Result<(), String> {
    blocking(move || {
        let _guard = lock_mutations()?;
        let profile_id = profile_id.trim().to_string();
        validate_profile_id(&profile_id)?;
        if recovered_profile_store(&settings::profiles_root())?
            .load(&profile_id)
            .map_err(|error| error.to_string())?
            .is_none()
        {
            return Err("Profile not found.".into());
        }
        settings::set_active_profile(&profile_id).map_err(|error| error.to_string())
    })
    .await
}

#[tauri::command]
pub async fn save_settings(
    mut settings: Settings,
    mut token_action: TokenAction,
) -> Result<SettingsView, String> {
    blocking(move || {
        let _guard = lock_mutations()?;
        managed_instance::migrate_direct_source_storage()?;
        if let TokenAction::Set { token } = &mut token_action {
            *token = token.trim().to_string();
            if token.is_empty() {
                return Err("GitHub token cannot be blank".into());
            }
        }
        let previous_settings = settings::load().map_err(|error| error.to_string())?;
        if settings.storage_path != previous_settings.storage_path {
            return Err(
                "Use the storage location controls to move Perfect Sync data safely.".into(),
            );
        }
        let mut ids = HashSet::new();
        let mut paths = Vec::<PathBuf>::new();
        for instance in &mut settings.game_instances {
            instance.id = instance.id.trim().to_string();
            instance.name = instance.name.trim().to_string();
            instance.path = instance.path.trim().to_string();
            if instance.id.is_empty() || !ids.insert(instance.id.clone()) {
                return Err("Every Among Us instance needs a unique id.".to_string());
            }
            if instance.name.is_empty() {
                instance.name = "Among Us".to_string();
            }
            let canonical = canonical_game_path(Path::new(&instance.path))?;
            validate_game_dir(&canonical)?;
            if paths.iter().any(|path| path == &canonical) {
                return Err("Every Among Us instance needs a unique folder.".to_string());
            }
            instance.path = canonical.to_string_lossy().into_owned();
            instance.executable_identity = game_executable_identity(&canonical);
            instance.arch = game::exe_arch(&canonical.join(process::GAME_EXE))
                .ok_or("Among Us executable architecture is unsupported")?;
            instance.runtime =
                compat::resolve_with_hint(&canonical, Some(instance.runtime)).runtime;
            let detected_store = game::store_for_path(&canonical, Store::Manual);
            if detected_store != Store::Manual {
                instance.store = detected_store;
            }
            instance.build = game::detect_build(&canonical);
            instance.writable = game::is_writable_game_dir(&canonical);
            let source = managed_instance::record_source(instance)?;
            instance.source_fingerprint = Some(source.record.fingerprint);
            instance.source_file_count = Some(source.record.file_count);
            instance.source_byte_count = Some(source.record.byte_count);
            paths.push(canonical);
        }
        let mut personal_sources = HashSet::new();
        for personal in &mut settings.personal_mods {
            personal.repo = resolver::parse_repo(personal.repo.trim())
                .ok_or("Every personal mod needs a valid GitHub repository.")?;
            personal.tag = personal.tag.trim().to_string();
            personal.asset = personal.asset.trim().to_string();
            personal.name = personal
                .name
                .take()
                .map(|name| name.trim().to_string())
                .filter(|name| !name.is_empty());
            if personal.tag.is_empty()
                || personal.tag.len() > 128
                || personal.asset.is_empty()
                || personal.asset.len() > 255
                || personal.asset.contains('/')
                || personal.asset.contains('\\')
                || personal.asset.chars().any(char::is_control)
            {
                return Err("Every personal mod needs a valid tag and release asset name.".into());
            }
            let identity = format!(
                "{}\0{}\0{}",
                personal.repo.to_ascii_lowercase(),
                personal.tag,
                personal.asset
            );
            if !personal_sources.insert(identity) {
                return Err("Personal mods cannot contain duplicate release assets.".into());
            }
        }
        let mut local_sources = HashSet::new();
        for local in &mut settings.personal_local_mods {
            local.path = local.path.trim().to_string();
            if !Path::new(&local.path).is_absolute() {
                return Err("Every local lobby default needs an absolute DLL path.".into());
            }
            let source_metadata =
                fs::symlink_metadata(&local.path).map_err(|error| error.to_string())?;
            if is_reparse(&source_metadata) || !source_metadata.is_file() {
                return Err("Every local lobby default must be a safe regular DLL file.".into());
            }
            let canonical = fs::canonicalize(&local.path)
                .map_err(|error| format!("Local lobby default is unavailable: {error}"))?;
            let metadata = fs::symlink_metadata(&canonical).map_err(|error| error.to_string())?;
            if is_reparse(&metadata) {
                return Err("Every local lobby default must be a safe regular DLL file.".into());
            }
            if !metadata.is_file() {
                return Err("Every local lobby default must be a regular DLL file.".into());
            }
            let file_name = canonical
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or("Local lobby default has no portable file name.")?;
            profile::validate_dll_name(file_name).map_err(|error| error.to_string())?;
            if !local_sources.insert(canonical.to_string_lossy().to_ascii_lowercase()) {
                return Err("Local lobby defaults cannot contain duplicate DLL files.".into());
            }
            local.path = canonical.to_string_lossy().into_owned();
            local.name = canonical
                .file_stem()
                .and_then(|name| name.to_str())
                .unwrap_or(file_name)
                .to_string();
        }
        for profile in store()?.list().map_err(|error| error.to_string())? {
            let Some(instance_id) = profile.game_instance_id.as_deref() else {
                if profile.mods.is_empty() {
                    continue;
                }
                return Err(format!(
                    "Profile {} has no saved game instance. Select an instance for that profile first.",
                    profile.name
                ));
            };
            let replacement = settings
                .game_instances
                .iter()
                .find(|instance| instance.id == instance_id)
                .ok_or_else(|| {
                    format!(
                        "Game instance {instance_id} is still used by profile {}. Reassign or delete that profile first.",
                        profile.name
                    )
                })?;
            let previous = previous_settings
                .game_instances
                .iter()
                .find(|instance| instance.id == instance_id);
            if !profile.mods.is_empty() {
                let previous = previous.ok_or_else(|| {
                    format!(
                        "Profile {} has resolved mods but its prior game instance is missing. Re-create/re-resolve the profile before changing instances.",
                        profile.name
                    )
                })?;
                if previous.arch != replacement.arch || previous.store != replacement.store {
                    return Err(format!(
                        "Profile {} already contains resolved mods. Keep its game instance on the same architecture and store, or create/re-resolve a profile for the new installation.",
                        profile.name
                    ));
                }
                if profile.game_build.is_some() && replacement.build != profile.game_build {
                    return Err(format!(
                        "Profile {} is pinned to Among Us build {}. Keep it on a source with that exact build, or create and re-resolve a profile for the new source.",

                        profile.name,
                        profile.game_build.as_deref().unwrap_or("unknown"),
                    ));
                }
            }
        }
        if let Some(active) = settings.active_profile.take() {
            let active = active.trim().to_string();
            validate_profile_id(&active)?;
            if store()?
                .load(&active)
                .map_err(|error| error.to_string())?
                .is_none()
            {
                return Err("Active profile does not exist.".into());
            }
            settings.active_profile = Some(active);
        }
        let was_logging = settings::support_logging_enabled();
        let logging_enabled = settings.support_logging;
        let view = settings::apply_transaction(&settings, &token_action)
            .map_err(|error| error.to_string())?;
        if was_logging && !logging_enabled {
            log::info!(
                target: "perfect_sync::support",
                "Diagnostic logging disabled by the user"
            );
        }
        settings::set_support_logging_enabled(logging_enabled);
        if !was_logging && logging_enabled {
            log::info!(
                target: "perfect_sync::support",
                "Diagnostic logging enabled; app_version={} os={} arch={}",
                env!("CARGO_PKG_VERSION"),
                std::env::consts::OS,
                std::env::consts::ARCH,
            );
        }
        Ok(view)
    })
    .await
}

fn storage_root_after_ambiguous_save(
    persisted_storage_path: Option<&str>,
    previous_storage_path: Option<&str>,
    target_storage_path: Option<&str>,
    current_root: &Path,
    target_root: &Path,
) -> Result<PathBuf, String> {
    if persisted_storage_path == target_storage_path {
        return Ok(target_root.to_path_buf());
    }
    if persisted_storage_path == previous_storage_path {
        return Ok(current_root.to_path_buf());
    }
    Err("persisted storage pointer matched neither the previous nor target location".into())
}
#[tauri::command]
pub async fn move_storage(
    storage_path: Option<String>,
    on_progress: Channel<OperationProgress>,
) -> Result<SettingsView, String> {
    blocking(move || {
        let _guard = lock_mutations()?;
        let _profile_guards = lock_all_profile_mutations()?;
        let _asset_cache_guards = lock_all_asset_caches()?;
        managed_workspaces_are_stopped()?;
        managed_instance::migrate_direct_source_storage()?;
        let reporter = ProgressReporter::new(on_progress);
        reporter.stage("preparing", "Validating the new storage location");

        let previous = settings::load().map_err(|error| error.to_string())?;
        let previous_storage_path = previous.storage_path.clone();
        let current_root = settings::managed_data_dir();
        let current_cache = settings::cache_dir();
        let default_root = settings::default_managed_data_dir();
        let app_data_root = settings::app_data_dir();
        let game_sources = previous
            .game_instances
            .iter()
            .map(|instance| PathBuf::from(&instance.path))
            .collect::<Vec<_>>();
        storage::retry_pending_storage_cleanup(&current_root, &game_sources).map_err(|error| {
            format!(
                "The previous storage cleanup is still pending. Close programs using the old storage and retry: {error}"
            )
        })?;
        let Some(target) = storage::resolve_target(
            storage_path.as_deref(),
            &current_root,
            &default_root,
            &app_data_root,
            &game_sources,
        )?
        else {
            return settings::view().map_err(|error| error.to_string());
        };

        reporter.stage(
            "copying",
            "Creating a verified copy of managed game data and caches",
        );
        let mut published = storage::copy_payload(
            &current_root,
            &current_cache,
            &target,
            |copied, total, message| reporter.transfer("copying", message, copied, Some(total)),
        )?;
        let mut updated = previous;
        updated.storage_path = target.configured_path.clone();
        if let Err(error) = settings::set_managed_data_dir(target.root.clone()) {
            let rollback = storage::rollback_published(&published).err();
            return Err(match rollback {
                Some(rollback) => {
                    format!("{error}; additionally could not remove the copied storage: {rollback}")
                }
                None => error.to_string(),
            });
        }
        if let Err(error) =
            storage::prepare_published_cleanup(&mut published, &current_root, &current_cache)
        {
            return match settings::set_managed_data_dir(current_root.clone()) {
                Ok(()) => {
                    let rollback = storage::rollback_published(&published).err();
                    Err(match rollback {
                        Some(rollback) => format!(
                            "{error}; additionally could not remove the copied storage: {rollback}"
                        ),
                        None => error,
                    })
                }
                Err(root_rollback) => {
                    let disarm = storage::disarm_published_cleanup(&published).err();
                    Err(match disarm {
                        Some(disarm) => format!(
                            "{error}; additionally could not restore the active storage path: {root_rollback}; automatic cleanup could not be disabled: {disarm}. Both storage copies were retained."
                        ),
                        None => format!(
                            "{error}; additionally could not restore the active storage path: {root_rollback}. Automatic cleanup was disabled and both storage copies were retained."
                        ),
                    })
                }
            };
        }
        if let Err(error) = settings::save(&updated) {
            let mut errors = vec![format!(
                "Could not confirm whether the new storage pointer was persisted: {error}"
            )];
            if let Err(disarm) = storage::disarm_published_cleanup(&published) {
                errors.push(format!(
                    "automatic cleanup could not be fully disarmed: {disarm}"
                ));
            }
            match settings::load() {
                Ok(persisted) => match storage_root_after_ambiguous_save(
                    persisted.storage_path.as_deref(),
                    previous_storage_path.as_deref(),
                    target.configured_path.as_deref(),
                    &current_root,
                    &target.root,
                ) {
                    Ok(authoritative_root) => {
                        if let Err(pointer_error) =
                            settings::set_managed_data_dir(authoritative_root)
                        {
                            errors.push(format!(
                                "the runtime storage pointer could not follow the persisted settings: {pointer_error}"
                            ));
                        }
                    }
                    Err(pointer_error) => errors.push(pointer_error),
                },
                Err(load_error) => errors.push(format!(
                    "the persisted storage pointer could not be reread: {load_error}"
                )),
            }
            errors.push(format!(
                "Both storage copies were retained at {} and {}",
                current_root.display(),
                target.root.display()
            ));
            return Err(errors.join("; "));
        }

        reporter.stage(
            "finalizing",
            "Switching Perfect Sync to the relocated storage",
        );
        let cleanup_errors = storage::commit_published_and_cleanup(
            &mut published,
            &current_root,
            &current_cache,
        );
        let mut view = settings::view().map_err(|error| error.to_string())?;
        if !cleanup_errors.is_empty() {
            view.storage_warning = Some(format!(
                "Storage moved successfully, but the old copy could not be removed completely: {}",
                cleanup_errors.join("; ")
            ));
        }
        Ok(view)
    })
    .await
}

fn copy_error_log(source: &Path, destination: &Path, expected: u64) -> Result<(), String> {
    let mut input = File::open(source)
        .map_err(|error| format!("Could not open the BepInEx error log: {error}"))?;
    AtomicFile::new(destination, AllowOverwrite)
        .write(|output| {
            let copied = io::copy(&mut Read::by_ref(&mut input).take(expected), output)?;
            if copied != expected {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "BepInEx error log was truncated while it was being exported",
                ));
            }
            output.flush()?;
            output.sync_all()
        })
        .map_err(|error| format!("Could not save the BepInEx error log: {error}"))
}

#[tauri::command]
pub async fn export_error_log(destination: String, profile_id: String) -> Result<String, String> {
    blocking(move || {
        validate_profile_id(&profile_id)?;
        let destination_path = PathBuf::from(&destination);
        if !destination_path.is_absolute()
            || destination_path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_none_or(|extension| !extension.eq_ignore_ascii_case("log"))
        {
            return Err("Choose an absolute .log destination for the BepInEx error log.".into());
        }
        let parent = destination_path
            .parent()
            .ok_or("The error log destination has no parent folder.")?;
        let parent_metadata = fs::symlink_metadata(parent)
            .map_err(|error| format!("Could not open the error log destination: {error}"))?;
        if is_reparse(&parent_metadata) || !parent_metadata.is_dir() {
            return Err("Choose a regular non-linked destination folder for the error log.".into());
        }
        if let Ok(metadata) = fs::symlink_metadata(&destination_path) {
            if is_reparse(&metadata) || !metadata.is_file() {
                return Err("The error log destination is not a regular file.".into());
            }
        }

        let _guard = lock_mutations()?;
        let source = managed_instance::workspace_game_dir(&profile_id)?
            .join("BepInEx")
            .join("LogOutput.log");
        let metadata = fs::symlink_metadata(&source).map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                "No BepInEx error log is available yet. Launch a managed profile, then try again."
                    .to_string()
            } else {
                format!("Could not inspect the BepInEx error log: {error}")
            }
        })?;
        if is_reparse(&metadata) || !metadata.is_file() || metadata.len() == 0 {
            return Err("No usable BepInEx error log is available yet. Launch a managed profile, then try again.".into());
        }
        if metadata.len() > MAX_ERROR_LOG_BYTES {
            return Err("The BepInEx error log is too large to export safely.".into());
        }
        if destination_path == source
            || (destination_path.exists()
                && fs::canonicalize(&destination_path).ok() == fs::canonicalize(&source).ok())
        {
            return Err("Choose a destination outside the managed game workspace.".into());
        }

        copy_error_log(&source, &destination_path, metadata.len())?;
        Ok(destination)
    })
    .await
}

#[tauri::command]
pub async fn game_running(profile_id: String) -> Result<bool, String> {
    blocking(move || {
        validate_profile_id(&profile_id)?;
        if launch_pending(&profile_id)? {
            return Ok(true);
        }
        let game_dir = managed_instance::workspace_game_dir(&profile_id)?;
        process::try_is_game_dir_running(&game_dir).map_err(|error| error.to_string())
    })
    .await
}

#[tauri::command]
pub async fn stop_game(profile_id: String) -> Result<bool, String> {
    blocking(move || {
        validate_profile_id(&profile_id)?;
        let game_dir = managed_instance::workspace_game_dir(&profile_id)?;
        let stopped = process::terminate_game_dir(&game_dir).map_err(|error| error.to_string())?;
        let launching = LAUNCH_PENDING
            .lock()
            .map_err(|_| "launch-session lock is poisoned".to_string())?
            .remove(&profile_id);
        Ok(stopped || launching)
    })
    .await
}

// ---------- catalog ----------

#[derive(Serialize, Deserialize, Clone)]
pub struct CatalogListItem {
    pub id: String,
    pub name: String,
    pub repo: String,
    pub summary: String,
    pub tags: Vec<ModTag>,
    pub latest: String,
    #[serde(default)]
    pub dependencies: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub provides: Vec<String>,
    #[serde(
        rename = "dependencyVersions",
        default,
        skip_serializing_if = "HashMap::is_empty"
    )]
    pub dependency_versions: HashMap<String, String>,
    #[serde(
        rename = "recommendedDependencies",
        default,
        skip_serializing_if = "Vec::is_empty"
    )]
    pub recommended_dependencies: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub included: Vec<String>,
    #[serde(default)]
    pub trust: Trust,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

fn catalog_item(entry: perfect_sync_core::catalog::CatalogEntry) -> CatalogListItem {
    let included = if is_tou_mira(&entry.id) {
        vec![
            "MiraAPI".into(),
            "Reactor".into(),
            "Mini.RegionInstall with the Town of Us server config".into(),
            "Town of Us cosmetics".into(),
        ]
    } else {
        Vec::new()
    };
    CatalogListItem {
        id: entry.id.clone(),
        name: entry.name,
        repo: entry.repo.unwrap_or(entry.id),
        summary: entry.summary,
        tags: entry.tags,
        dependencies: entry.dependencies,
        provides: entry.provides,
        dependency_versions: entry.dependency_versions,
        recommended_dependencies: entry.recommended_dependencies,
        included,
        latest: String::new(),
        trust: Trust::Flagged,
        extra: HashMap::new(),
    }
}

fn apply_authoritative_trust(list: &mut [CatalogListItem]) {
    let bundled = bundled_catalog();
    for item in list {
        item.trust = bundled
            .get(&item.id)
            .filter(|entry| entry.repo.as_deref().unwrap_or(&entry.id) == item.repo)
            .map(|entry| entry.trust)
            .unwrap_or(Trust::Flagged);
    }
}

#[derive(Serialize, Deserialize)]
struct CatalogEnvelope {
    #[serde(default = "catalog_envelope_version")]
    version: u32,
    display: Vec<CatalogListItem>,
    #[serde(default)]
    order_policy_version: u32,
    #[serde(default)]
    hosted_ids: Vec<String>,
    #[serde(default)]
    hidden_hosted_ids: Vec<String>,
    #[serde(default)]
    hosted_catalog: Option<Catalog>,
}

fn catalog_envelope_version() -> u32 {
    1
}

const CATALOG_ORDER_POLICY_VERSION: u32 = 1;

fn validate_catalog_list(list: &mut [CatalogListItem]) -> Result<(), String> {
    if serde_json::to_vec(list)
        .map_err(|error| error.to_string())?
        .len() as u64
        > MAX_USER_CATALOG_BYTES
    {
        return Err("user catalog exceeds its size limit".into());
    }
    let mut ids = HashSet::new();
    for item in list.iter() {
        if item.id.trim().is_empty() || !ids.insert(item.id.to_ascii_lowercase()) {
            return Err("user catalog contains an invalid or duplicate id".into());
        }
        resolver::parse_repo(&item.repo)
            .ok_or_else(|| format!("user catalog contains invalid repository {}", item.repo))?;
    }
    apply_authoritative_trust(list);
    Ok(())
}

fn load_catalog_state() -> Result<CatalogEnvelope, String> {
    let path = settings::user_catalog_path();
    let stored = read_bounded(&path, MAX_CATALOG_ENVELOPE_BYTES)?;
    if let Some(bytes) = stored.as_deref() {
        if let Ok(mut envelope) = serde_json::from_slice::<CatalogEnvelope>(bytes) {
            if envelope.version != catalog_envelope_version() {
                return Err("unsupported user catalog envelope version".into());
            }
            if envelope.order_policy_version < CATALOG_ORDER_POLICY_VERSION {
                apply_default_catalog_order(&mut envelope.display);
                envelope.order_policy_version = CATALOG_ORDER_POLICY_VERSION;
            }
            validate_catalog_list(&mut envelope.display)?;
            if let Some(hosted) = envelope.hosted_catalog.take() {
                envelope.hosted_catalog = Some(validate_persisted_catalog(hosted)?);
            }
            return Ok(envelope);
        }
    }
    let legacy = legacy_catalog()?;
    let legacy_ids: Vec<String> = legacy.mods.iter().map(|entry| entry.id.clone()).collect();
    let mut display = match stored {
        Some(bytes) => serde_json::from_slice::<Vec<CatalogListItem>>(&bytes)
            .map_err(|error| format!("invalid user catalog: {error}"))?,
        None => legacy.mods.iter().cloned().map(catalog_item).collect(),
    };
    apply_default_catalog_order(&mut display);
    let mut state = CatalogEnvelope {
        version: catalog_envelope_version(),
        display,
        order_policy_version: CATALOG_ORDER_POLICY_VERSION,
        hosted_ids: legacy_ids,
        hidden_hosted_ids: Vec::new(),
        hosted_catalog: Some(legacy),
    };
    validate_catalog_list(&mut state.display)?;
    Ok(state)
}

fn display_catalog() -> Result<Vec<CatalogListItem>, String> {
    Ok(load_catalog_state()?.display)
}

fn serialized_catalog_state(state: &CatalogEnvelope) -> Result<Vec<u8>, String> {
    let mut bytes = serde_json::to_vec(state).map_err(|error| error.to_string())?;
    if serde_json::to_vec(&state.display)
        .map_err(|error| error.to_string())?
        .len() as u64
        > MAX_USER_CATALOG_BYTES
    {
        return Err("user catalog exceeds its size limit".into());
    }
    bytes.push(b'\n');
    if bytes.len() as u64 > MAX_CATALOG_ENVELOPE_BYTES {
        return Err("catalog persistence envelope exceeds its size limit".into());
    }
    Ok(bytes)
}

fn save_catalog_state(state: &CatalogEnvelope) -> Result<(), String> {
    atomic_write(
        &settings::user_catalog_path(),
        &serialized_catalog_state(state)?,
    )
}

fn ensure_display_catalog_state(
    state: &mut CatalogEnvelope,
    requested_repo: &str,
    canonical_id: &str,
    effective_repo: &str,
    name: &str,
    summary: String,
    tags: Vec<ModTag>,
) {
    state
        .hidden_hosted_ids
        .retain(|hidden| !hidden.eq_ignore_ascii_case(canonical_id));
    let mut canonical = None;
    state.display.retain(|item| {
        if item.id.eq_ignore_ascii_case(canonical_id) {
            if canonical.is_none() {
                canonical = Some(item.clone());
            }
            return false;
        }
        !item.id.eq_ignore_ascii_case(requested_repo)
            && !item.repo.eq_ignore_ascii_case(effective_repo)
    });
    let mut item = canonical.unwrap_or_else(|| CatalogListItem {
        id: canonical_id.to_string(),
        name: name.to_string(),
        repo: effective_repo.to_string(),
        summary,
        tags,
        latest: String::new(),
        dependencies: Vec::new(),
        provides: Vec::new(),
        dependency_versions: HashMap::new(),
        recommended_dependencies: Vec::new(),
        included: Vec::new(),
        trust: Trust::Flagged,
        extra: HashMap::new(),
    });
    item.id = canonical_id.to_string();
    item.repo = effective_repo.to_string();
    state.display.push(item);
}

fn ensure_display_catalog(
    requested_repo: &str,
    canonical_id: &str,
    effective_repo: &str,
    name: &str,
    summary: String,
    tags: Vec<ModTag>,
) -> Result<(), String> {
    let mut state = load_catalog_state()?;
    ensure_display_catalog_state(
        &mut state,
        requested_repo,
        canonical_id,
        effective_repo,
        name,
        summary,
        tags,
    );
    save_catalog_state(&state)
}

fn reconcile_hosted(mut state: CatalogEnvelope, hosted: &Catalog) -> CatalogEnvelope {
    let old_hosted: HashSet<String> = state
        .hosted_ids
        .iter()
        .map(|id| id.to_ascii_lowercase())
        .collect();
    let new_hosted: HashMap<String, _> = hosted
        .mods
        .iter()
        .map(|entry| (entry.id.to_ascii_lowercase(), entry))
        .collect();
    state
        .hidden_hosted_ids
        .retain(|id| new_hosted.contains_key(&id.to_ascii_lowercase()));
    let hidden: HashSet<String> = state
        .hidden_hosted_ids
        .iter()
        .map(|id| id.to_ascii_lowercase())
        .collect();
    let mut seen = HashSet::new();
    let mut output = Vec::with_capacity(state.display.len() + hosted.mods.len());
    for mut current in state.display {
        let folded = current.id.to_ascii_lowercase();
        if let Some(entry) = new_hosted.get(&folded) {
            seen.insert(folded.clone());
            if hidden.contains(&folded) {
                continue;
            }
            let mut replacement = catalog_item((*entry).clone());
            replacement.latest = std::mem::take(&mut current.latest);
            replacement.extra = std::mem::take(&mut current.extra);
            output.push(replacement);
        } else if !old_hosted.contains(&folded) {
            seen.insert(folded);
            output.push(current);
        }
    }
    for entry in &hosted.mods {
        let folded = entry.id.to_ascii_lowercase();
        if seen.insert(folded.clone()) && !hidden.contains(&folded) {
            output.push(catalog_item(entry.clone()));
        }
    }
    apply_bundled_display_policy(&mut output);
    apply_authoritative_trust(&mut output);
    apply_default_catalog_order(&mut output);
    state.display = output;
    state.hosted_ids = hosted.mods.iter().map(|entry| entry.id.clone()).collect();
    state.hosted_catalog = Some(hosted.clone());
    state
}

#[tauri::command]
pub async fn get_catalog() -> Result<Vec<CatalogListItem>, String> {
    blocking(|| {
        let _guard = lock_mutations()?;
        display_catalog()
    })
    .await
}

#[tauri::command]
pub async fn add_catalog_mod(
    repo: String,
    name: Option<String>,
) -> Result<Vec<CatalogListItem>, String> {
    blocking(move || {
        let _guard = lock_mutations()?;
        let repo = resolver::parse_repo(&repo).ok_or("invalid repo or URL")?;
        let catalog = catalog()?;
        let entry = catalog_entry_for(&catalog, &repo).cloned();
        let canonical_id = entry
            .as_ref()
            .map(|entry| entry.id.clone())
            .unwrap_or_else(|| repo.clone());
        let effective_repo = entry
            .as_ref()
            .map(|entry| entry.repo.clone().unwrap_or_else(|| entry.id.clone()))
            .unwrap_or_else(|| repo.clone());
        let display_name = name
            .map(|name| name.trim().to_string())
            .filter(|name| !name.is_empty())
            .or_else(|| entry.as_ref().map(|entry| entry.name.clone()))
            .unwrap_or_else(|| repo.clone());
        let summary = entry
            .as_ref()
            .map(|entry| entry.summary.clone())
            .unwrap_or_default();
        let tags = entry
            .as_ref()
            .map(|entry| entry.tags.clone())
            .unwrap_or_default();
        ensure_display_catalog(
            &repo,
            &canonical_id,
            &effective_repo,
            &display_name,
            summary,
            tags,
        )?;
        display_catalog()
    })
    .await
}

#[tauri::command]
pub async fn remove_catalog_mod(id: String) -> Result<Vec<CatalogListItem>, String> {
    blocking(move || {
        let _guard = lock_mutations()?;
        let mut state = load_catalog_state()?;
        state
            .display
            .retain(|item| !item.id.eq_ignore_ascii_case(&id));
        if state
            .hosted_ids
            .iter()
            .any(|hosted| hosted.eq_ignore_ascii_case(&id))
            && !state
                .hidden_hosted_ids
                .iter()
                .any(|hidden| hidden.eq_ignore_ascii_case(&id))
        {
            state.hidden_hosted_ids.push(id);
        }
        save_catalog_state(&state)?;
        Ok(state.display)
    })
    .await
}

#[tauri::command]
pub async fn reorder_catalog(ids: Vec<String>) -> Result<Vec<CatalogListItem>, String> {
    blocking(move || {
        let _guard = lock_mutations()?;
        let mut state = load_catalog_state()?;
        let current = &state.display;
        let mut seen = HashSet::new();
        let mut output = Vec::with_capacity(current.len());
        for id in ids {
            if !seen.insert(id.to_ascii_lowercase()) {
                return Err("catalog order contains duplicate ids".into());
            }
            let item = current
                .iter()
                .find(|item| item.id == id)
                .ok_or_else(|| format!("unknown catalog id {id}"))?;
            output.push(item.clone());
        }
        for item in current {
            if seen.insert(item.id.to_ascii_lowercase()) {
                output.push(item.clone());
            }
        }
        state.display = output;
        save_catalog_state(&state)?;
        Ok(state.display)
    })
    .await
}

#[tauri::command]
pub async fn refresh_catalog() -> Result<usize, String> {
    blocking(|| {
        let _guard = lock_mutations()?;
        let text = http()?
            .get_text(DEFAULT_CATALOG_URL)
            .map_err(|error| error.to_string())?;
        if text.len() as u64 > MAX_CATALOG_BYTES {
            return Err("hosted catalog exceeds its size limit".into());
        }
        let hosted = parse(&text).map_err(|error| format!("invalid catalog: {error}"))?;
        let state = reconcile_hosted(load_catalog_state()?, &hosted);
        save_catalog_state(&state)
            .map_err(|error| format!("could not publish hosted catalog: {error}"))?;
        Ok(hosted.mods.len())
    })
    .await
}

// ---------- profiles ----------

#[tauri::command]
pub async fn list_profiles() -> Result<Vec<ProfileRecord>, String> {
    blocking(|| {
        let _guard = lock_mutations()?;
        store()?.list().map_err(|error| error.to_string())
    })
    .await
}

#[tauri::command]
pub async fn save_profile(mut profile: ProfileRecord) -> Result<ProfileRecord, String> {
    blocking(move || {
        let _guard = lock_profile_mutation(&profile.id)?;
        validate_profile_id(&profile.id)?;
        profile.name = profile.name.trim().to_string();
        profile.crew_color = profile.crew_color.trim().to_string();
        if profile.name.is_empty() || profile.crew_color.is_empty() {
            return Err("profile name and crew color are required".into());
        }
        let saved = settings::load().map_err(|error| error.to_string())?;
        if profile.game_instance_id.is_none() && !saved.game_instances.is_empty() {
            return Err("choose and save an Among Us instance for this profile".into());
        }
        let proposed_instance = profile
            .game_instance_id
            .as_deref()
            .map(|instance_id| {
                selected_profile_instance(&saved, Some(instance_id))
            })
            .transpose()?;
        if let Some(existing) = store()?
            .load(&profile.id)
            .map_err(|error| error.to_string())?
        {
            if !existing.mods.is_empty() {
                let proposed_instance = proposed_instance.ok_or(
                    "This populated profile needs a saved compatible Among Us instance before it can be changed.",
                )?;
                let existing_instance =
                    selected_profile_instance(&saved, existing.game_instance_id.as_deref())
                        .map_err(|_| {
                            "The populated profile's prior game instance is missing, so its asset architecture cannot be verified. Create a new profile and re-resolve its mods."
                                .to_string()
                        })?;
                if existing_instance.arch != proposed_instance.arch
                    || existing_instance.store != proposed_instance.store
                {
                    return Err(
                        "This profile already contains architecture/store-specific assets. Keep it on a compatible Among Us instance, or create a new profile and re-resolve its mods."
                            .into(),
                    );
                }
                if existing.game_build.is_some() && proposed_instance.build != existing.game_build {
                    return Err(
                        "This profile is pinned to a different Among Us build. Keep it on a source with that exact build, or create and re-resolve a profile for the new source."
                            .into(),
                    );
                }
            }
            profile.game_build = existing.game_build;
            profile.mods = existing.mods;
        } else if !profile.mods.is_empty() {
            return Err("new profiles cannot contain backend-owned mods".into());
        }
        store()?.save(&profile).map_err(|error| error.to_string())?;
        Ok(profile)
    })
    .await
}

#[tauri::command]
pub async fn delete_profile(id: String) -> Result<(), String> {
    blocking(move || {
        let _guard = lock_profile_mutation(&id)?;
        validate_profile_id(&id)?;
        workspace_is_stopped(&id)?;
        let mut saved = settings::load().map_err(|error| error.to_string())?;
        let original = saved.clone();
        let clears_active = saved
            .active_profile
            .as_deref()
            .is_some_and(|active| active == id.as_str());
        if clears_active {
            saved.active_profile = None;
            settings::save(&saved).map_err(|error| error.to_string())?;
        }
        if let Err(error) = delete_profile_transaction(&settings::profiles_root(), &id) {
            if clears_active {
                return match settings::save(&original) {
                    Ok(()) => Err(error),
                    Err(rollback) => Err(format!(
                        "{error}; additionally could not restore the active-profile setting: {rollback}"
                    )),
                };
            }
            return Err(error);
        }
        Ok(())
    })
    .await
}

// ---------- lobby codes ----------

#[tauri::command]
pub async fn encode_lobby_code(profile: ProfileRecord) -> Result<String, String> {
    blocking(move || {
        let _guard = lock_profile_mutation(&profile.id)?;
        let profile_store = store()?;
        let mut authoritative = profile_store
            .load(&profile.id)
            .map_err(|error| error.to_string())?
            .ok_or("profile not found")?;
        let saved = settings::load().map_err(|error| error.to_string())?;
        let instance =
            selected_profile_instance(&saved, authoritative.game_instance_id.as_deref())?;
        authoritative.game_build = game::detect_build(Path::new(&instance.path));
        authoritative.levelimposter_maps = list_levelimposter_maps_impl(&authoritative.id)?;
        profile_store
            .save(&authoritative)
            .map_err(|error| error.to_string())?;
        codec::encode(&profile::to_manifest(&authoritative)).map_err(|error| error.to_string())
    })
    .await
}

#[tauri::command]
pub fn preview_code(code: String, installed: Vec<(String, String)>) -> Result<Preview, String> {
    preview(&code, &bundled_catalog(), &installed).map_err(|error| error.to_string())
}

// ---------- release/file picker ----------

#[tauri::command]
pub async fn list_releases(repo: String) -> Result<Vec<Release>, String> {
    blocking(move || {
        let repo = resolver::parse_repo(&repo).ok_or("invalid repo or URL")?;
        fetch_releases_bounded(&http()?, &repo, 20)
    })
    .await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModInstallOption {
    tag: String,
    asset_name: String,
    size: u64,
}

fn install_options(
    releases: Vec<Release>,
    rules: &AssetRules,
    arch: &str,
) -> Vec<ModInstallOption> {
    let mut options = Vec::new();
    for release in releases {
        let preferred =
            resolver::pick_asset(&release, rules, arch).map(|asset| asset.name.as_str());
        let mut assets: Vec<_> = release
            .assets
            .iter()
            .filter(|asset| {
                let lower = asset.name.to_ascii_lowercase();
                lower.ends_with(".dll")
                    || (lower.ends_with(".zip") && preferred == Some(asset.name.as_str()))
            })
            .collect();
        assets.sort_by_key(|asset| preferred != Some(asset.name.as_str()));
        options.extend(assets.into_iter().map(|asset| ModInstallOption {
            tag: release.tag.clone(),
            asset_name: asset.name.clone(),
            size: asset.size.bytes(),
        }));
    }
    options
}

fn install_options_for_profile(
    releases: Vec<Release>,
    repo: &str,
    rules: &AssetRules,
    arch: &str,
    store: Store,
    runtime: Runtime,
) -> Result<Vec<ModInstallOption>, String> {
    if !is_tou_mira(repo) {
        return Ok(install_options(releases, rules, arch));
    }
    let mut options = Vec::new();
    for release in releases {
        if let Some(asset) = pick_profile_asset(&release, repo, rules, arch, store, runtime)? {
            let asset_name = asset.name.clone();
            let size = asset.size.bytes();
            options.push(ModInstallOption {
                tag: release.tag,
                asset_name,
                size,
            });
        }
    }
    Ok(options)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModInstallSelection {
    id: String,
    repo: String,
    name: String,
    tag: String,
    asset_name: String,
    managed: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationProgress {
    phase: String,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    bytes_received: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    bytes_total: Option<u64>,
}

#[derive(Clone)]
struct ProgressReporter {
    channel: Channel<OperationProgress>,
}

impl ProgressReporter {
    fn new(channel: Channel<OperationProgress>) -> Self {
        Self { channel }
    }

    fn stage(&self, phase: &str, message: impl Into<String>) {
        let message = message.into();
        log::info!(
            target: "perfect_sync::support",
            "operation phase={phase}: {}",
            support_log_message(message.clone())
        );
        let _ = self.channel.send(OperationProgress {
            phase: phase.to_string(),
            message,
            bytes_received: None,
            bytes_total: None,
        });
    }

    fn download(&self, message: &str, bytes_received: u64, bytes_total: Option<u64>) {
        let _ = self.channel.send(OperationProgress {
            phase: "downloading".into(),
            message: message.to_string(),
            bytes_received: Some(bytes_received),
            bytes_total,
        });
    }

    fn transfer(&self, phase: &str, message: &str, bytes_received: u64, bytes_total: Option<u64>) {
        let _ = self.channel.send(OperationProgress {
            phase: phase.to_string(),
            message: message.to_string(),
            bytes_received: Some(bytes_received),
            bytes_total,
        });
    }
}

struct ProgressHttp {
    inner: UreqHttp,
    reporter: ProgressReporter,
}

impl ProgressHttp {
    fn new(inner: UreqHttp, reporter: ProgressReporter) -> Self {
        Self { inner, reporter }
    }
}

impl Http for ProgressHttp {
    fn get_text(&self, url: &str) -> Result<String, perfect_sync_core::resolver::ResolveError> {
        self.inner.get_text(url)
    }
    fn get_text_with_url(
        &self,
        url: &str,
    ) -> Result<perfect_sync_core::resolver::TextResponse, perfect_sync_core::resolver::ResolveError>
    {
        self.inner.get_text_with_url(url)
    }

    fn head(
        &self,
        url: &str,
    ) -> Result<perfect_sync_core::resolver::HeadResponse, perfect_sync_core::resolver::ResolveError>
    {
        self.inner.head(url)
    }

    fn get_bytes(&self, url: &str) -> Result<Vec<u8>, perfect_sync_core::resolver::ResolveError> {
        let mut bytes = Vec::new();
        self.download_to(url, &mut bytes, &mut |_, _| {})?;
        Ok(bytes)
    }

    fn download_to(
        &self,
        url: &str,
        output: &mut dyn Write,
        _on_progress: &mut dyn FnMut(u64, Option<u64>),
    ) -> Result<u64, perfect_sync_core::resolver::ResolveError> {
        let label = url::Url::parse(url)
            .ok()
            .and_then(|parsed| {
                parsed
                    .path_segments()
                    .and_then(|mut segments| segments.next_back())
                    .map(str::to_string)
            })
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "download".into())
            .replace("%20", " ");
        let message = format!("Downloading {label}");
        let mut last_emit = None;
        let mut last_received = 0_u64;
        let mut last_total = None;
        let result = self.inner.download_to(url, output, &mut |received, total| {
            let now = Instant::now();
            let should_emit = received == 0
                || total.is_some_and(|expected| received >= expected)
                || last_emit.is_none_or(|last: Instant| {
                    now.duration_since(last) >= Duration::from_millis(100)
                });
            last_received = received;
            last_total = total;
            if should_emit {
                self.reporter.download(&message, received, total);
                last_emit = Some(now);
            }
        });
        if let Ok(received) = result {
            if received != last_received {
                self.reporter.download(&message, received, last_total);
            }
            Ok(received)
        } else {
            result
        }
    }
}

#[tauri::command]
pub async fn list_install_options(
    repo: String,
    profile_id: String,
    limit: Option<u32>,
) -> Result<Vec<ModInstallOption>, String> {
    blocking(move || {
        let limit = limit.unwrap_or(10);
        if !(1..=50).contains(&limit) {
            return Err("release option limit must be between 1 and 50".into());
        }
        validate_profile_id(&profile_id)?;
        let arch = profile_arch(&profile_id)?;
        let (store, runtime) = profile_store_runtime(&profile_id)?;
        let repo = resolver::parse_repo(&repo).ok_or("invalid repo or URL")?;
        let catalog = catalog()?;
        let rules = catalog_entry_for(&catalog, &repo)
            .map(|entry| entry.asset_rules.clone())
            .unwrap_or_else(default_rules);
        let started = Instant::now();
        let releases = fetch_releases_bounded(&http()?, &repo, limit)?;
        let release_count = releases.len();
        let options = install_options_for_profile(releases, &repo, &rules, &arch, store, runtime)?;
        log_perf("release_options", started, release_count, 0);
        Ok(options)
    })
    .await
}

#[tauri::command]
pub async fn list_tou_setup_options(
    arch: String,
    store: Store,
    runtime: Runtime,
) -> Result<Vec<ModInstallOption>, String> {
    blocking(move || {
        if !matches!(arch.as_str(), "x86" | "x64") {
            return Err("Among Us executable architecture is unsupported".into());
        }
        let catalog = catalog()?;
        let entry = catalog
            .get(TOU_MIRA_ID)
            .ok_or("Town of Us is missing from the trusted catalog")?;
        let repo = entry.repo.as_deref().unwrap_or(&entry.id);
        let releases = fetch_releases_bounded(&http()?, repo, 50)?;
        install_options_for_profile(releases, repo, &entry.asset_rules, &arch, store, runtime)
    })
    .await
}

#[tauri::command]
pub async fn install_assets(
    profile_id: String,
    selections: Vec<ModInstallSelection>,
    confirmed: bool,
    on_progress: Channel<OperationProgress>,
) -> Result<ProfileRecord, String> {
    require_manual_install_confirmation(confirmed)?;
    blocking(move || {
        let _guard = lock_profile_mutation(&profile_id)?;
        require_profile_source_for_install(&profile_id)?;
        let reporter = ProgressReporter::new(on_progress);
        install_assets_impl(profile_id, selections, &reporter)
    })
    .await
}

#[tauri::command]
pub async fn install_local_mod(profile_id: String, path: String) -> Result<ProfileRecord, String> {
    blocking(move || {
        let _guard = lock_profile_mutation(&profile_id)?;
        require_profile_source_for_install(&profile_id)?;
        install_local_mod_impl(&settings::profiles_root(), &profile_id, Path::new(&path))
    })
    .await
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LevelImposterMap {
    id: String,
    name: String,
    author_name: String,
    description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    thumbnail_url: Option<String>,
}

#[derive(Deserialize)]
struct LevelImposterCallback<T> {
    v: u32,
    #[serde(default)]
    error: String,
    data: Option<T>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LevelImposterMapMetadata {
    id: String,
    name: String,
    #[serde(default)]
    author_name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    is_public: bool,
    #[serde(default, rename = "downloadURL")]
    download_url: Option<String>,
    #[serde(default, rename = "thumbnailURL")]
    thumbnail_url: Option<String>,
}

#[derive(Deserialize)]
struct LevelImposterSearchResponse {
    hits: Vec<LevelImposterSearchHit>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LevelImposterSearchHit {
    #[serde(rename = "objectID")]
    id: String,
    name: String,
    #[serde(default)]
    author_name: String,
    #[serde(default)]
    description: String,
    #[serde(default, rename = "thumbnailURL")]
    thumbnail_url: Option<String>,
}

#[tauri::command]
pub async fn search_levelimposter_maps(query: String) -> Result<Vec<LevelImposterMap>, String> {
    blocking(move || search_levelimposter_maps_impl(&query)).await
}

#[tauri::command]
pub async fn fetch_levelimposter_banner(url: String) -> Result<String, String> {
    blocking(move || levelimposter_banner_data_url(&http()?, &url)).await
}

#[tauri::command]
pub async fn list_levelimposter_maps(profile_id: String) -> Result<Vec<String>, String> {
    blocking(move || list_levelimposter_maps_impl(&profile_id)).await
}

#[tauri::command]
pub async fn install_levelimposter_maps(
    profile_id: String,
    map_ids: Vec<String>,
    on_progress: Channel<OperationProgress>,
) -> Result<ProfileRecord, String> {
    blocking(move || {
        let _guard = lock_profile_mutation(&profile_id)?;
        require_profile_source_for_install(&profile_id)?;
        let reporter = ProgressReporter::new(on_progress);
        install_levelimposter_maps_impl(profile_id, map_ids, &reporter)
    })
    .await
}
#[tauri::command]
pub async fn remove_levelimposter_maps(
    profile_id: String,
    map_ids: Vec<String>,
) -> Result<ProfileRecord, String> {
    blocking(move || {
        let _guard = lock_profile_mutation(&profile_id)?;
        require_profile_source_for_install(&profile_id)?;
        remove_levelimposter_maps_impl(profile_id, map_ids)
    })
    .await
}

/// Install a specific release asset (chosen by the user) into a profile.
#[tauri::command]
pub async fn install_asset(
    profile_id: String,
    repo: String,
    tag: String,
    asset_name: String,
    arch: String,
    confirmed: bool,
    on_progress: Channel<OperationProgress>,
) -> Result<ProfileRecord, String> {
    require_manual_install_confirmation(confirmed)?;
    blocking(move || {
        let _guard = lock_profile_mutation(&profile_id)?;
        require_profile_source_for_install(&profile_id)?;
        let reporter = ProgressReporter::new(on_progress);
        install_asset_impl(profile_id, repo, tag, asset_name, arch, &reporter)
    })
    .await
}

fn require_manual_install_confirmation(confirmed: bool) -> Result<(), String> {
    if confirmed {
        Ok(())
    } else {
        Err("Confirm the exact repository, release tag, and asset before installing.".into())
    }
}

fn catalog_entry_for<'a>(
    catalog: &'a Catalog,
    identity: &str,
) -> Option<&'a perfect_sync_core::catalog::CatalogEntry> {
    catalog.get(identity).or_else(|| {
        catalog.mods.iter().find(|entry| {
            entry
                .repo
                .as_deref()
                .is_some_and(|repo| repo.eq_ignore_ascii_case(identity))
        })
    })
}
fn installed_dependency_provider<'a>(
    record: &'a ProfileRecord,
    catalog: &Catalog,
    dependency_id: &str,
) -> Option<&'a InstalledMod> {
    record
        .mods
        .iter()
        .filter(|installed| installed.enabled)
        .find(|installed| {
            catalog_entry_for(catalog, &installed.package_id)
                .or_else(|| {
                    installed
                        .repo
                        .as_deref()
                        .and_then(|repo| catalog_entry_for(catalog, repo))
                })
                .is_some_and(|entry| {
                    entry
                        .provides
                        .iter()
                        .any(|provided| provided.eq_ignore_ascii_case(dependency_id))
                })
        })
}
fn provided_dependency_ids(record: &ProfileRecord, catalog: &Catalog) -> HashSet<String> {
    record
        .mods
        .iter()
        .filter(|installed| installed.enabled)
        .filter_map(|installed| {
            catalog_entry_for(catalog, &installed.package_id).or_else(|| {
                installed
                    .repo
                    .as_deref()
                    .and_then(|repo| catalog_entry_for(catalog, repo))
            })
        })
        .flat_map(|entry| entry.provides.iter())
        .map(|provided| provided.to_ascii_lowercase())
        .collect()
}
fn catalog_provided_dependency_ids(catalog: &Catalog, identities: &[String]) -> HashSet<String> {
    identities
        .iter()
        .filter_map(|identity| catalog_entry_for(catalog, identity))
        .flat_map(|entry| entry.provides.iter())
        .map(|provided| provided.to_ascii_lowercase())
        .collect()
}

fn is_managed_dependency(root_id: &str, candidate_id: &str) -> bool {
    !root_id.eq_ignore_ascii_case(candidate_id)
}

fn selected_dependencies(
    catalog: &Catalog,
    selected: &[String],
) -> Result<HashSet<String>, String> {
    let selected_folded: HashSet<String> =
        selected.iter().map(|id| id.to_ascii_lowercase()).collect();
    let mut dependencies = HashSet::new();
    for root in selected {
        for candidate in deps::resolve(catalog, std::slice::from_ref(root))
            .map_err(|error| error.to_string())?
            .ordered
        {
            if !candidate.eq_ignore_ascii_case(root)
                && selected_folded.contains(&candidate.to_ascii_lowercase())
            {
                dependencies.insert(candidate.to_ascii_lowercase());
            }
        }
    }
    Ok(dependencies)
}

fn validate_authoritative_dependencies(
    catalog: &Catalog,
    explicit_roots: &[String],
) -> Result<(), String> {
    let bundled = bundled_catalog();
    for root in explicit_roots {
        let resolved = deps::resolve(catalog, std::slice::from_ref(root))
            .map_err(|error| error.to_string())?;
        for candidate in &resolved.ordered {
            let is_root = candidate.eq_ignore_ascii_case(root);
            let hosted = catalog
                .get(candidate)
                .ok_or_else(|| format!("catalog dependency {candidate} is missing"))?;
            let Some(authoritative) = bundled.get(&hosted.id) else {
                if is_root {
                    // A custom explicit root was already manually confirmed by the user.
                    continue;
                }
                return Err(format!(
                    "Catalog dependency {} is not authorized by the bundled catalog.",
                    hosted.id
                ));
            };
            let hosted_repo = hosted.repo.as_deref().unwrap_or(&hosted.id);
            let authoritative_repo = authoritative.repo.as_deref().unwrap_or(&authoritative.id);
            if !hosted_repo.eq_ignore_ascii_case(authoritative_repo) {
                let role = if is_root { "root" } else { "dependency" };
                return Err(format!(
                    "Catalog {role} {} does not match its bundled authoritative identity.",
                    hosted.id
                ));
            }
        }
    }
    Ok(())
}

fn explicit_catalog_roots(record: &ProfileRecord, catalog: &Catalog) -> Vec<String> {
    record
        .mods
        .iter()
        .filter(|installed| !installed.managed)
        .filter_map(|installed| {
            catalog_entry_for(catalog, &installed.package_id).map(|entry| entry.id.clone())
        })
        .collect()
}

fn normalize_dependency_ownership(
    stage_root: &Path,
    profile_id: &str,
    record: &mut ProfileRecord,
    catalog: &Catalog,
) -> Result<(), String> {
    let explicit_roots: Vec<String> = record
        .mods
        .iter()
        .filter(|installed| !installed.managed)
        .filter_map(|installed| {
            catalog_entry_for(catalog, &installed.package_id).map(|entry| entry.id.clone())
        })
        .collect();
    let required: HashSet<String> = if explicit_roots.is_empty() {
        HashSet::new()
    } else {
        deps::resolve(catalog, &explicit_roots)
            .map_err(|error| error.to_string())?
            .ordered
            .into_iter()
            .map(|id| id.to_ascii_lowercase())
            .collect()
    };
    let roots: HashSet<String> = explicit_roots
        .into_iter()
        .map(|id| id.to_ascii_lowercase())
        .collect();
    let mut retained = Vec::with_capacity(record.mods.len());
    for mut installed in record.mods.drain(..) {
        let canonical = catalog_entry_for(catalog, &installed.package_id)
            .map(|entry| entry.id.to_ascii_lowercase());
        match canonical {
            Some(id) if roots.contains(&id) => {
                installed.managed = false;
                retained.push(installed);
            }
            Some(id) if required.contains(&id) => {
                installed.managed = true;
                retained.push(installed);
            }
            _ if installed.managed => {
                if let Some(file) = installed.file.as_deref() {
                    profile::remove_plugin(stage_root, profile_id, file)
                        .map_err(|error| error.to_string())?;
                }
            }
            _ => retained.push(installed),
        }
    }
    record.mods = retained;
    Ok(())
}

fn mod_position(record: &ProfileRecord, package_id: &str) -> Result<usize, String> {
    record
        .mods
        .iter()
        .position(|installed| installed.package_id.eq_ignore_ascii_case(package_id))
        .ok_or_else(|| "mod not found".to_string())
}

fn validate_mod_toggle(
    record: &ProfileRecord,
    catalog: &Catalog,
    package_id: &str,
) -> Result<usize, String> {
    if let Some(provider) = installed_dependency_provider(record, catalog, package_id) {
        return Err(format!(
            "{package_id} is supplied by the enabled {} bundle and cannot be toggled separately.",
            provider.name
        ));
    }
    mod_position(record, package_id)
}

fn remove_mod_from_record(
    stage_root: &Path,
    profile_id: &str,
    record: &mut ProfileRecord,
    catalog: &Catalog,
    package_id: &str,
) -> Result<(), String> {
    if let Some(provider) = installed_dependency_provider(record, catalog, package_id) {
        return Err(format!(
            "{package_id} is supplied by the enabled {} bundle and cannot be removed separately.",
            provider.name
        ));
    }
    let position = mod_position(record, package_id)?;
    let removed = record.mods.remove(position);
    if is_tou_mira(&removed.package_id) {
        profile::remove_tou_bundle(stage_root, profile_id).map_err(|error| error.to_string())?;
    }
    if let Some(file) = removed.file {
        profile::remove_plugin(stage_root, profile_id, &file).map_err(|error| error.to_string())?;
    }
    normalize_dependency_ownership(stage_root, profile_id, record, catalog)
}

#[derive(Clone, Copy)]
struct ReleaseAssetTarget<'a> {
    rules: Option<&'a AssetRules>,
    arch: &'a str,
    store: Store,
    runtime: Runtime,
}

fn selected_release_asset(
    http: &dyn Http,
    repo: &str,
    tag: &str,
    asset_name: &str,
    target: ReleaseAssetTarget<'_>,
) -> Result<ResolvedDownload, String> {
    let release =
        resolver::fetch_release_by_tag(http, repo, tag).map_err(|error| error.to_string())?;
    if is_tou_mira(repo) {
        let rules = target
            .rules
            .ok_or("Town of Us must use its authoritative catalog rules")?;
        let asset = pick_profile_asset(
            &release,
            repo,
            rules,
            target.arch,
            target.store,
            target.runtime,
        )?
        .ok_or_else(|| format!("Town of Us {tag} has no compatible full package"))?;
        return resolver::resolved_asset(http, &release, asset).map_err(|error| error.to_string());
    }

    let lower = asset_name.to_ascii_lowercase();
    if !lower.ends_with(".dll") && !lower.ends_with(".zip") {
        return Err("Only .dll files and catalog-selected .zip packages can be installed.".into());
    }
    let asset = release
        .assets
        .iter()
        .find(|asset| asset.name == asset_name)
        .ok_or("selected file not found in that release")?;
    if lower.ends_with(".zip") {
        let selected = target
            .rules
            .and_then(|rules| resolver::pick_asset(&release, rules, target.arch))
            .is_some_and(|selected| selected.name == asset.name);
        if !selected {
            return Err(
                "Only the catalog-selected ZIP package for this game architecture can be installed."
                    .into(),
            );
        }
    }
    resolver::resolved_asset(http, &release, asset).map_err(|error| error.to_string())
}
fn staged_plugin_names(stage_root: &Path, profile_id: &str) -> Result<HashSet<String>, String> {
    let directory = stage_root.join(profile_id).join("BepInEx").join("plugins");
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(HashSet::new()),
        Err(error) => return Err(error.to_string()),
    };
    let mut names = HashSet::new();
    for entry in entries {
        let entry = entry.map_err(|error| error.to_string())?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| "profile plugins contain a non-Unicode filename")?;
        let metadata = fs::symlink_metadata(entry.path()).map_err(|error| error.to_string())?;
        if is_reparse(&metadata) {
            return Err("profile plugins contain an unsupported filesystem entry".into());
        }
        if metadata.is_dir() && name.eq_ignore_ascii_case("LevelImposter") {
            continue;
        }
        if !metadata.is_file() {
            return Err("profile plugins contain an unsupported filesystem entry".into());
        }
        if !names.insert(name.to_ascii_lowercase()) {
            return Err("profile plugins contain case-colliding filenames".into());
        }
    }
    Ok(names)
}

struct InstallContext<'a> {
    stage_root: &'a Path,
    profile_id: &'a str,
    http: &'a dyn Http,
    catalog: &'a Catalog,
    arch: &'a str,
    store: Store,
    runtime: Runtime,
}

struct InstallRequest {
    package_id: String,
    name: String,
    repo: String,
    tags: Vec<ModTag>,
    managed: bool,
    resolved: ResolvedDownload,
}

fn tou_bundle_dependency(package_id: &str) -> bool {
    TOU_BUNDLED_DEPENDENCY_IDS
        .iter()
        .any(|dependency| dependency.eq_ignore_ascii_case(package_id))
}

fn profile_has_tou_mira(record: &ProfileRecord) -> bool {
    record
        .mods
        .iter()
        .any(|installed| is_tou_mira(&installed.package_id))
}

fn profile_uses_tou_mira(record: &ProfileRecord) -> bool {
    record
        .mods
        .iter()
        .any(|installed| installed.enabled && is_tou_mira(&installed.package_id))
}

fn safe_nonempty_plugin(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.len() > 0 && !is_reparse(&metadata))
}

fn reuse_installed_dependency(
    context: &InstallContext<'_>,
    record: &mut ProfileRecord,
    id: &str,
    requirements: &[String],
) -> Result<bool, String> {
    if installed_dependency_provider(record, context.catalog, id).is_some() {
        return Ok(true);
    }
    let Some(position) = record
        .mods
        .iter()
        .position(|installed| installed.package_id.eq_ignore_ascii_case(id))
    else {
        return Ok(false);
    };
    let installed = record.mods[position].clone();
    if !requirements.is_empty()
        && !perfect_sync_core::version::satisfies_all(&installed.version, requirements)
    {
        if !installed.managed {
            let name = context
                .catalog
                .get(id)
                .map_or(id, |entry| entry.name.as_str());
            return Err(format!(
                "{} {} does not satisfy required version {}.",
                name,
                installed.version,
                requirements.join(", ")
            ));
        }
        return Ok(false);
    }
    let Some(file) = installed.file.as_deref() else {
        return Ok(false);
    };
    if profile::validate_dll_name(file).is_err() {
        return Ok(false);
    }
    if !installed.enabled {
        profile::set_plugin_enabled(context.stage_root, context.profile_id, file, true)
            .map_err(|error| error.to_string())?;
        record.mods[position].enabled = true;
    }
    let plugins = context
        .stage_root
        .join(context.profile_id)
        .join("BepInEx")
        .join("plugins");
    if !safe_nonempty_plugin(&plugins.join(file)) {
        return Ok(false);
    }
    if is_tou_mira(&installed.package_id) {
        let Ok(files) = profile::tou_bundle_files(context.stage_root, context.profile_id) else {
            return Ok(false);
        };
        return Ok(!files.is_empty()
            && files.iter().all(|relative| {
                safe_nonempty_plugin(
                    &context
                        .stage_root
                        .join(context.profile_id)
                        .join("BepInEx")
                        .join(relative),
                )
            }));
    }
    Ok(true)
}

fn tou_shadowed_plugin_files(record: &ProfileRecord) -> Vec<String> {
    if !profile_uses_tou_mira(record) {
        return Vec::new();
    }
    let mut names: Vec<String> = TOU_BUNDLED_PLUGIN_FILES
        .iter()
        .map(|name| (*name).to_string())
        .collect();
    for file in record
        .mods
        .iter()
        .filter(|installed| tou_bundle_dependency(&installed.package_id))
        .filter_map(|installed| installed.file.as_deref())
    {
        if !names.iter().any(|name| name.eq_ignore_ascii_case(file)) {
            names.push(file.to_string());
        }
    }
    names
}

fn install_tou_record(
    context: &InstallContext<'_>,
    record: &mut ProfileRecord,
    request: InstallRequest,
) -> Result<(), String> {
    if !request
        .resolved
        .asset_name
        .to_ascii_lowercase()
        .ends_with(".zip")
    {
        return Err("Town of Us must be installed from its complete release ZIP".into());
    }
    let bytes =
        download_resolved(context.http, &request.resolved).map_err(|error| error.to_string())?;
    let previous = record.mods.iter().position(|installed| {
        installed
            .package_id
            .eq_ignore_ascii_case(&request.package_id)
    });
    let previous_versions = previous
        .map(|position| record.mods[position].versions.clone())
        .unwrap_or_default();
    let previous_file = previous.and_then(|position| record.mods[position].file.clone());

    profile::remove_tou_bundle(context.stage_root, context.profile_id)
        .map_err(|error| error.to_string())?;
    if let Some(file) = previous_file {
        profile::remove_plugin(context.stage_root, context.profile_id, &file)
            .map_err(|error| error.to_string())?;
    }

    let file =
        profile::install_tou_bundle_zip_bytes(context.stage_root, context.profile_id, &bytes)
            .map_err(|error| error.to_string())?;
    cache_tou_package(
        &request.resolved.version,
        &request.resolved.asset_name,
        &bytes,
    )?;
    let previous = record.mods.iter().position(|installed| {
        installed
            .package_id
            .eq_ignore_ascii_case(&request.package_id)
    });
    let preserve_explicit_root =
        request.managed && previous.is_some_and(|position| !record.mods[position].managed);
    let mut versions = previous_versions;
    if !versions.contains(&request.resolved.version) {
        versions.insert(0, request.resolved.version.clone());
    }
    let installed = InstalledMod {
        package_id: request.package_id,
        name: request.name,
        repo: Some(request.repo),
        version: request.resolved.version.clone(),
        versions,
        enabled: true,
        source: ModSource::Github,
        tags: request.tags,
        managed: request.managed && !preserve_explicit_root,
        update: None,
        file: Some(file),
        asset: Some(request.resolved.asset_name),
    };
    if let Some(position) = previous {
        record.mods[position] = installed;
    } else {
        record.mods.push(installed);
    }
    Ok(())
}

fn install_record(
    context: &InstallContext<'_>,
    record: &mut ProfileRecord,
    request: InstallRequest,
) -> Result<(), String> {
    if is_tou_mira(&request.package_id) {
        return install_tou_record(context, record, request);
    }
    if let Some(provider) =
        installed_dependency_provider(record, context.catalog, &request.package_id)
    {
        return Err(format!(
            "{} is supplied by the installed {} bundle and must not be installed separately",
            request.name, provider.name
        ));
    }
    let InstallRequest {
        package_id,
        name,
        repo,
        tags,
        managed,
        resolved,
    } = request;
    let preserve_explicit_root = managed
        && record.mods.iter().any(|installed| {
            installed.package_id.eq_ignore_ascii_case(&package_id) && !installed.managed
        });
    if let Some(existing) = record
        .mods
        .iter()
        .find(|installed| installed.package_id.eq_ignore_ascii_case(&package_id))
        .and_then(|installed| installed.file.clone())
    {
        profile::remove_plugin(context.stage_root, context.profile_id, &existing)
            .map_err(|error| error.to_string())?;
    }
    let preexisting = staged_plugin_names(context.stage_root, context.profile_id)?;
    let expected_dll = context
        .catalog
        .get(&package_id)
        .and_then(|entry| entry.asset_rules.dll_name.as_deref());
    let file = install_resolved(
        context.stage_root,
        context.profile_id,
        context.http,
        &resolved,
        expected_dll,
    )?;
    let lower = file.to_ascii_lowercase();
    if preexisting.contains(&lower) || preexisting.contains(&format!("{lower}.disabled")) {
        return Err(format!(
            "plugin file {file} would overwrite a file not owned by this package"
        ));
    }
    if record.mods.iter().any(|installed| {
        !installed.package_id.eq_ignore_ascii_case(&package_id)
            && installed
                .file
                .as_deref()
                .is_some_and(|owned| owned.eq_ignore_ascii_case(&file))
    }) {
        return Err(format!(
            "plugin file {file} is already owned by another installed package"
        ));
    }
    let previous = record
        .mods
        .iter()
        .position(|installed| installed.package_id.eq_ignore_ascii_case(&package_id));
    let mut versions = previous
        .map(|position| record.mods[position].versions.clone())
        .unwrap_or_default();
    if !versions.contains(&resolved.version) {
        versions.insert(0, resolved.version.clone());
    }
    let installed = InstalledMod {
        package_id,
        name,
        repo: Some(repo),
        version: resolved.version.clone(),
        versions,
        enabled: true,
        source: ModSource::Github,
        tags,
        managed: managed && !preserve_explicit_root,
        update: None,
        file: Some(file),
        asset: Some(resolved.asset_name),
    };
    if let Some(position) = previous {
        record.mods[position] = installed;
    } else {
        record.mods.push(installed);
    }
    Ok(())
}

fn install_catalog_latest(
    context: &InstallContext<'_>,
    record: &mut ProfileRecord,
    id: &str,
    managed: bool,
    requirements: &[String],
) -> Result<(), String> {
    let entry = context
        .catalog
        .get(id)
        .ok_or("catalog dependency is missing")?;
    if managed && reuse_installed_dependency(context, record, &entry.id, requirements)? {
        return Ok(());
    }
    let repo = entry
        .repo
        .clone()
        .or_else(|| resolver::parse_repo(&entry.id))
        .ok_or_else(|| format!("cannot resolve source for {}", entry.id))?;
    let resolved = if requirements.is_empty() {
        resolve_profile_latest(
            context.http,
            &repo,
            &entry.asset_rules,
            context.arch,
            context.store,
            context.runtime,
        )?
    } else {
        let tags = resolver::fetch_release_tags(context.http, &repo, 20)
            .map_err(|error| error.to_string())?;
        let mut selected = None;
        for tag in tags {
            if !perfect_sync_core::version::satisfies_all(&tag, requirements) {
                continue;
            }
            let release = resolver::fetch_release_by_tag(context.http, &repo, &tag)
                .map_err(|error| error.to_string())?;
            let Some(asset) = pick_profile_asset(
                &release,
                &repo,
                &entry.asset_rules,
                context.arch,
                context.store,
                context.runtime,
            )?
            else {
                continue;
            };
            selected = Some(
                resolver::resolved_asset(context.http, &release, asset)
                    .map_err(|error| error.to_string())?,
            );
            break;
        }
        selected.ok_or_else(|| {
            format!(
                "{} has no recent release satisfying {}.",
                entry.name,
                requirements.join(", ")
            )
        })?
    };
    install_record(
        context,
        record,
        InstallRequest {
            package_id: entry.id.clone(),
            name: entry.name.clone(),
            repo,
            tags: entry.tags.clone(),
            managed,
            resolved,
        },
    )
}

fn install_local_mod_into_record(
    profiles_root: &Path,
    profile_id: &str,
    record: &mut ProfileRecord,
    source: &Path,
) -> Result<(), String> {
    if !source.is_absolute() {
        return Err("Choose an absolute path to a local .dll file.".into());
    }
    let file_name = source
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("Local mod path has no portable file name.")?;
    profile::validate_dll_name(file_name).map_err(|error| error.to_string())?;
    let display_name = source
        .file_stem()
        .and_then(|name| name.to_str())
        .ok_or("Local mod path has no portable file name.")?
        .to_string();
    let package_id = format!("local/{}", file_name.to_ascii_lowercase());
    if record.mods.iter().any(|installed| {
        !installed.package_id.eq_ignore_ascii_case(&package_id)
            && installed
                .file
                .as_deref()
                .is_some_and(|owned| owned.eq_ignore_ascii_case(file_name))
    }) {
        return Err(format!(
            "plugin file {file_name} is already owned by another installed package"
        ));
    }

    let previous = record
        .mods
        .iter()
        .position(|installed| installed.package_id.eq_ignore_ascii_case(&package_id));
    if let Some(existing) = previous.and_then(|position| record.mods[position].file.as_deref()) {
        profile::remove_plugin(profiles_root, profile_id, existing)
            .map_err(|error| error.to_string())?;
    }
    let preexisting = staged_plugin_names(profiles_root, profile_id)?;
    let folded_file = file_name.to_ascii_lowercase();
    if preexisting.contains(&folded_file)
        || preexisting.contains(&format!("{folded_file}.disabled"))
    {
        return Err(format!(
            "plugin file {file_name} would overwrite a file not owned by this package"
        ));
    }

    profile::install_plugin_dll(profiles_root, profile_id, source)
        .map_err(|error| error.to_string())?;
    let installed = InstalledMod {
        package_id,
        name: display_name,
        repo: None,
        version: "local".into(),
        versions: vec!["local".into()],
        enabled: true,
        source: ModSource::File,
        tags: Vec::new(),
        managed: false,
        update: None,
        file: Some(file_name.to_string()),
        asset: Some(file_name.to_string()),
    };
    if let Some(position) = previous {
        record.mods[position] = installed;
    } else {
        record.mods.push(installed);
    }
    Ok(())
}

fn install_local_mod_impl(
    profiles_root: &Path,
    profile_id: &str,
    source: &Path,
) -> Result<ProfileRecord, String> {
    validate_profile_id(profile_id)?;
    profile_transaction(profiles_root, profile_id, |stage_root, stage_store| {
        let mut record = stage_store
            .load(profile_id)
            .map_err(|error| error.to_string())?
            .ok_or("profile not found")?;
        install_local_mod_into_record(stage_root, profile_id, &mut record, source)?;
        stage_store
            .save(&record)
            .map_err(|error| error.to_string())?;
        Ok(record)
    })
}

fn install_asset_impl(
    profile_id: String,
    repo: String,
    tag: String,
    asset_name: String,
    arch: String,
    reporter: &ProgressReporter,
) -> Result<ProfileRecord, String> {
    reporter.stage("preparing", "Checking profile and dependencies");
    validate_profile_id(&profile_id)?;
    let _ = arch;
    let arch = profile_arch(&profile_id)?;
    let (store, runtime) = profile_store_runtime(&profile_id)?;
    let repo = resolver::parse_repo(&repo).ok_or("invalid repo or URL")?;
    let catalog = catalog()?;
    let root_entry = catalog_entry_for(&catalog, &repo).cloned();
    let (ordered, requirements) = match root_entry.as_ref() {
        Some(entry) => {
            let plan = deps::resolve(&catalog, std::slice::from_ref(&entry.id))
                .map_err(|error| error.to_string())?;
            (plan.ordered, plan.requirements)
        }
        None => (Vec::new(), HashMap::new()),
    };
    let root = settings::profiles_root();
    profile_transaction(&root, &profile_id, |stage_root, stage_store| {
        let mut record = stage_store
            .load(&profile_id)
            .map_err(|error| error.to_string())?
            .ok_or("profile not found")?;
        let mut explicit_roots = explicit_catalog_roots(&record, &catalog);
        if let Some(entry) = root_entry.as_ref() {
            if !explicit_roots
                .iter()
                .any(|root| root.eq_ignore_ascii_case(&entry.id))
            {
                explicit_roots.push(entry.id.clone());
            }
        }
        validate_authoritative_dependencies(&catalog, &explicit_roots)?;
        reporter.stage("resolving", "Resolving exact release files");
        let http = ProgressHttp::new(http()?, reporter.clone());
        let install = InstallContext {
            stage_root,
            profile_id: &profile_id,
            http: &http,
            catalog: &catalog,
            arch: &arch,
            store,
            runtime,
        };
        for dependency in ordered.iter().filter(|id| {
            root_entry
                .as_ref()
                .is_none_or(|root| is_managed_dependency(&root.id, id))
        }) {
            install_catalog_latest(
                &install,
                &mut record,
                dependency,
                true,
                requirements.get(dependency).map_or(&[][..], Vec::as_slice),
            )?;
        }
        let resolved = selected_release_asset(
            &http,
            &repo,
            &tag,
            &asset_name,
            ReleaseAssetTarget {
                rules: root_entry.as_ref().map(|entry| &entry.asset_rules),
                arch: &arch,
                store,
                runtime,
            },
        )?;
        install_record(
            &install,
            &mut record,
            InstallRequest {
                package_id: root_entry
                    .as_ref()
                    .map(|entry| entry.id.clone())
                    .unwrap_or_else(|| repo.clone()),
                name: root_entry
                    .as_ref()
                    .map(|entry| entry.name.clone())
                    .unwrap_or_else(|| repo.clone()),
                repo: repo.clone(),
                tags: root_entry
                    .as_ref()
                    .map(|entry| entry.tags.clone())
                    .unwrap_or_default(),
                managed: false,
                resolved,
            },
        )?;
        reporter.stage("finalizing", "Verifying and saving the profile");
        normalize_dependency_ownership(stage_root, &profile_id, &mut record, &catalog)?;
        stage_store
            .save(&record)
            .map_err(|error| error.to_string())?;
        Ok(record)
    })
}

fn install_assets_impl(
    profile_id: String,
    selections: Vec<ModInstallSelection>,
    reporter: &ProgressReporter,
) -> Result<ProfileRecord, String> {
    reporter.stage(
        "preparing",
        format!(
            "Checking {} selected file{}",
            selections.len(),
            if selections.len() == 1 { "" } else { "s" }
        ),
    );
    validate_profile_id(&profile_id)?;
    if selections.is_empty() || selections.len() > 64 {
        return Err("Select between 1 and 64 mods and dependencies.".into());
    }
    let arch = profile_arch(&profile_id)?;
    let (store, runtime) = profile_store_runtime(&profile_id)?;
    let catalog = catalog()?;
    let mut seen = HashSet::with_capacity(selections.len());
    let mut prepared = Vec::with_capacity(selections.len());
    let mut selected_catalog_roots = Vec::new();
    for selection in selections {
        let repo = resolver::parse_repo(&selection.repo).ok_or("invalid repo or URL")?;
        if !seen.insert(repo.to_ascii_lowercase()) {
            return Err(format!("Repository {repo} was selected more than once."));
        }
        let name = selection.name.trim();
        if name.is_empty() || name.len() > 128 || name.chars().any(char::is_control) {
            return Err("Mod names must be 1..=128 non-control characters.".into());
        }
        let catalog_entry = catalog_entry_for(&catalog, &selection.id)
            .or_else(|| catalog_entry_for(&catalog, &repo))
            .cloned();
        if let Some(entry) = catalog_entry.as_ref() {
            let authoritative_repo = entry.repo.as_deref().unwrap_or(&entry.id);
            if !authoritative_repo.eq_ignore_ascii_case(&repo) {
                return Err(format!(
                    "Catalog entry {} does not match repository {repo}.",
                    entry.id
                ));
            }
            if !selection.managed {
                selected_catalog_roots.push(entry.id.clone());
            }
        } else if selection.managed {
            return Err("Only bundled catalog entries can be auto-managed dependencies.".into());
        }
        prepared.push((selection, repo, catalog_entry));
    }

    let plan = if selected_catalog_roots.is_empty() {
        deps::Resolved {
            ordered: Vec::new(),
            requirements: HashMap::new(),
        }
    } else {
        deps::resolve(&catalog, &selected_catalog_roots).map_err(|error| error.to_string())?
    };
    let ordered = &plan.ordered;
    let selected_root_ids: HashSet<String> = selected_catalog_roots
        .iter()
        .map(|id| id.to_ascii_lowercase())
        .collect();
    let selected_provided_ids = catalog_provided_dependency_ids(&catalog, &selected_catalog_roots);
    let allowed_ids: HashSet<String> = ordered.iter().map(|id| id.to_ascii_lowercase()).collect();
    let selected_catalog_ids: HashSet<String> = prepared
        .iter()
        .filter_map(|(_, _, entry)| entry.as_ref())
        .map(|entry| entry.id.to_ascii_lowercase())
        .collect();
    for (selection, _, entry) in &prepared {
        if selection.managed {
            let id = entry.as_ref().unwrap().id.to_ascii_lowercase();
            if selected_root_ids.contains(&id) || !allowed_ids.contains(&id) {
                return Err(format!(
                    "{} is not a dependency of the selected mods.",
                    entry.as_ref().unwrap().name
                ));
            }
        }
    }
    let omitted_dependencies: Vec<String> = ordered
        .iter()
        .filter(|id| {
            let folded = id.to_ascii_lowercase();
            !selected_root_ids.contains(&folded) && !selected_catalog_ids.contains(&folded)
        })
        .cloned()
        .collect();
    let order: HashMap<String, usize> = ordered
        .iter()
        .enumerate()
        .map(|(position, id)| (id.to_ascii_lowercase(), position))
        .collect();
    prepared.sort_by_key(|(_, _, entry)| {
        entry
            .as_ref()
            .and_then(|entry| order.get(&entry.id.to_ascii_lowercase()))
            .copied()
            .unwrap_or(usize::MAX)
    });

    let root = settings::profiles_root();
    profile_transaction(&root, &profile_id, |stage_root, stage_store| {
        let mut record = stage_store
            .load(&profile_id)
            .map_err(|error| error.to_string())?
            .ok_or("profile not found")?;
        let previous_roots = explicit_catalog_roots(&record, &catalog);
        let other_roots: Vec<String> = previous_roots
            .iter()
            .filter(|id| !selected_root_ids.contains(&id.to_ascii_lowercase()))
            .cloned()
            .collect();
        let other_required: HashSet<String> = if other_roots.is_empty() {
            HashSet::new()
        } else {
            deps::resolve(&catalog, &other_roots)
                .map_err(|error| error.to_string())?
                .ordered
                .into_iter()
                .map(|id| id.to_ascii_lowercase())
                .collect()
        };
        let mut explicit_roots = previous_roots;
        for id in &selected_catalog_roots {
            if !explicit_roots
                .iter()
                .any(|root| root.eq_ignore_ascii_case(id))
            {
                explicit_roots.push(id.clone());
            }
        }
        validate_authoritative_dependencies(&catalog, &explicit_roots)?;
        reporter.stage("resolving", "Resolving exact releases and dependencies");
        let http = ProgressHttp::new(http()?, reporter.clone());
        let install = InstallContext {
            stage_root,
            profile_id: &profile_id,
            http: &http,
            catalog: &catalog,
            arch: &arch,
            store,
            runtime,
        };
        for (selection, repo, catalog_entry) in &prepared {
            let requirements = catalog_entry
                .as_ref()
                .and_then(|entry| plan.requirements.get(&entry.id))
                .map_or(&[][..], Vec::as_slice);
            if selection.managed {
                let entry = catalog_entry.as_ref().unwrap();
                if selected_provided_ids.contains(&entry.id.to_ascii_lowercase())
                    || reuse_installed_dependency(&install, &mut record, &entry.id, requirements)?
                {
                    continue;
                }
            }
            if !requirements.is_empty()
                && !perfect_sync_core::version::satisfies_all(&selection.tag, requirements)
            {
                return Err(format!(
                    "{} {} does not satisfy required version {}.",
                    catalog_entry.as_ref().unwrap().name,
                    selection.tag,
                    requirements.join(", ")
                ));
            }
            let resolved = selected_release_asset(
                &http,
                repo,
                &selection.tag,
                &selection.asset_name,
                ReleaseAssetTarget {
                    rules: catalog_entry.as_ref().map(|entry| &entry.asset_rules),
                    arch: &arch,
                    store,
                    runtime,
                },
            )?;
            install_record(
                &install,
                &mut record,
                InstallRequest {
                    package_id: catalog_entry
                        .as_ref()
                        .map(|entry| entry.id.clone())
                        .unwrap_or_else(|| repo.clone()),
                    name: catalog_entry
                        .as_ref()
                        .map(|entry| entry.name.clone())
                        .unwrap_or_else(|| selection.name.trim().to_string()),
                    repo: repo.clone(),
                    tags: catalog_entry
                        .as_ref()
                        .map(|entry| entry.tags.clone())
                        .unwrap_or_default(),
                    managed: selection.managed,
                    resolved,
                },
            )?;
        }
        reporter.stage("finalizing", "Verifying files and saving the profile");
        normalize_dependency_ownership(stage_root, &profile_id, &mut record, &catalog)?;
        for omitted in &omitted_dependencies {
            if other_required.contains(&omitted.to_ascii_lowercase()) {
                continue;
            }
            let position = record.mods.iter().position(|installed| {
                installed.managed
                    && catalog_entry_for(&catalog, &installed.package_id)
                        .is_some_and(|entry| entry.id.eq_ignore_ascii_case(omitted))
            });
            if let Some(position) = position {
                let removed = record.mods.remove(position);
                if let Some(file) = removed.file {
                    profile::remove_plugin(stage_root, &profile_id, &file)
                        .map_err(|error| error.to_string())?;
                }
            }
        }
        stage_store
            .save(&record)
            .map_err(|error| error.to_string())?;
        Ok(record)
    })
}

fn levelimposter_callback<T>(text: &str) -> Result<T, String>
where
    T: for<'de> Deserialize<'de>,
{
    let response: LevelImposterCallback<T> = serde_json::from_str(text)
        .map_err(|error| format!("Invalid LevelImposter response: {error}"))?;
    if response.v != 1 {
        return Err(format!(
            "Unsupported LevelImposter API version {}.",
            response.v
        ));
    }
    if !response.error.is_empty() {
        return Err(response.error);
    }
    response
        .data
        .ok_or_else(|| "LevelImposter returned no map data.".to_string())
}

fn levelimposter_banner_data_url(http: &dyn Http, raw_url: &str) -> Result<String, String> {
    let parsed = url::Url::parse(raw_url).map_err(|_| "Invalid LevelImposter banner URL.")?;
    let official_path = parsed
        .path()
        .starts_with("/v0/b/levelimposter-347807.appspot.com/o/maps%2F");
    if parsed.scheme() != "https"
        || parsed.host_str() != Some("firebasestorage.googleapis.com")
        || !official_path
    {
        return Err("LevelImposter banner URL is not from the official map bucket.".into());
    }
    let bytes = http
        .get_bytes(parsed.as_str())
        .map_err(|error| error.to_string())?;
    if bytes.is_empty() || bytes.len() > MAX_LEVELIMPOSTER_BANNER_BYTES {
        return Err("LevelImposter banner exceeds the size limit.".into());
    }
    let media_type = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        "image/png"
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        "image/jpeg"
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        "image/webp"
    } else {
        return Err("LevelImposter banner is not a supported image.".into());
    };
    Ok(format!(
        "data:{media_type};base64,{}",
        BASE64_STANDARD.encode(bytes)
    ))
}

fn search_levelimposter_maps_impl(query: &str) -> Result<Vec<LevelImposterMap>, String> {
    let query = query.trim();
    if query.len() > 128 || query.chars().any(char::is_control) {
        return Err("Map searches must be at most 128 non-control characters.".into());
    }
    let http = http()?;
    if query.is_empty() {
        let text = http
            .get_text(&format!("{LEVELIMPOSTER_API}/maps/top"))
            .map_err(|error| error.to_string())?;
        let maps: Vec<LevelImposterMapMetadata> = levelimposter_callback(&text)?;
        return Ok(maps
            .into_iter()
            .filter(|map| valid_levelimposter_map_id(&map.id))
            .take(40)
            .map(|map| LevelImposterMap {
                id: map.id,
                name: map.name,
                author_name: map.author_name,
                description: map.description,
                thumbnail_url: map.thumbnail_url,
            })
            .collect());
    }

    let mut url = url::Url::parse(LEVELIMPOSTER_ALGOLIA_URL).map_err(|error| error.to_string())?;
    url.query_pairs_mut()
        .append_pair("query", query)
        .append_pair("hitsPerPage", "40")
        .append_pair("x-algolia-application-id", LEVELIMPOSTER_ALGOLIA_APP_ID)
        .append_pair("x-algolia-api-key", LEVELIMPOSTER_ALGOLIA_SEARCH_KEY);
    let text = http
        .get_text(url.as_str())
        .map_err(|error| error.to_string())?;
    let response: LevelImposterSearchResponse = serde_json::from_str(&text)
        .map_err(|error| format!("Invalid LevelImposter search response: {error}"))?;
    Ok(response
        .hits
        .into_iter()
        .filter(|hit| valid_levelimposter_map_id(&hit.id))
        .take(40)
        .map(|hit| LevelImposterMap {
            id: hit.id,
            name: hit.name,
            author_name: hit.author_name,
            description: hit.description,
            thumbnail_url: hit.thumbnail_url,
        })
        .collect())
}

fn list_levelimposter_maps_impl(profile_id: &str) -> Result<Vec<String>, String> {
    validate_profile_id(profile_id)?;
    recovered_profile_store(&settings::profiles_root())?
        .load(profile_id)
        .map_err(|error| error.to_string())?
        .ok_or("profile not found")?;
    let directory = settings::profiles_root()
        .join(profile_id)
        .join("BepInEx")
        .join("plugins")
        .join("LevelImposter");
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.to_string()),
    };
    let mut ids = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| error.to_string())?;
        let metadata = fs::symlink_metadata(entry.path()).map_err(|error| error.to_string())?;
        if is_reparse(&metadata) {
            return Err("LevelImposter map folder contains a link or reparse point.".into());
        }
        if !metadata.is_file() {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        let Some(id) = name.strip_suffix(".lim2") else {
            continue;
        };
        if valid_levelimposter_map_id(id) {
            ids.push(id.to_ascii_lowercase());
        }
    }
    ids.sort();
    ids.dedup();
    Ok(ids)
}

fn valid_levelimposter_map_download_url(parsed: &url::Url, id: &str) -> bool {
    if parsed.scheme() != "https"
        || parsed.host_str() != Some("storage.googleapis.com")
        || parsed.port().is_some()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.fragment().is_some()
    {
        return false;
    }

    let Some(segments) = parsed.path_segments() else {
        return false;
    };
    let segments: Vec<_> = segments.collect();
    if segments.len() < 3
        || !matches!(
            segments[0].to_ascii_lowercase().as_str(),
            "levelimposter" | "levelimposter-347807.appspot.com"
        )
        || !segments[1].eq_ignore_ascii_case("maps")
    {
        return false;
    }

    let Some(file_name) = segments.last() else {
        return false;
    };
    file_name.eq_ignore_ascii_case(&format!("{id}.lim"))
        || file_name.eq_ignore_ascii_case(&format!("{id}.lim2"))
}

fn levelimposter_map_download(http: &dyn Http, id: &str) -> Result<(String, Vec<u8>), String> {
    let text = http
        .get_text(&format!("{LEVELIMPOSTER_API}/map/{id}"))
        .map_err(|error| error.to_string())?;
    let metadata: LevelImposterMapMetadata = levelimposter_callback(&text)?;
    if !metadata.id.eq_ignore_ascii_case(id) || !metadata.is_public {
        return Err(format!("LevelImposter map {id} is not public."));
    }
    let download_url = metadata
        .download_url
        .ok_or_else(|| format!("LevelImposter map {} has no download.", metadata.name))?;
    let parsed = url::Url::parse(&download_url).map_err(|_| "Invalid map download URL.")?;
    if !valid_levelimposter_map_download_url(&parsed, id) {
        return Err("LevelImposter returned an untrusted map download URL.".into());
    }
    let bytes = http
        .get_bytes(parsed.as_str())
        .map_err(|error| error.to_string())?;
    if bytes.is_empty() || bytes.len() as u64 > MAX_LEVELIMPOSTER_MAP_BYTES {
        return Err(format!(
            "LevelImposter map {} has an invalid download size.",
            metadata.name
        ));
    }
    Ok((id.to_ascii_lowercase(), bytes))
}

fn download_levelimposter_maps(
    http: &dyn Http,
    ids: &[String],
) -> Result<Vec<(String, Vec<u8>)>, String> {
    let mut downloads = Vec::with_capacity(ids.len());
    let mut total_bytes = 0_u64;
    for id in ids {
        let download = levelimposter_map_download(http, id)?;
        total_bytes = total_bytes
            .checked_add(download.1.len() as u64)
            .filter(|total| *total <= MAX_LEVELIMPOSTER_MAP_TOTAL_BYTES)
            .ok_or("Selected LevelImposter maps exceed the batch size limit.")?;
        downloads.push(download);
    }
    Ok(downloads)
}

fn replace_profile_levelimposter_maps(
    profiles_root: &Path,
    profile_id: &str,
    previously_owned: &[String],
    downloads: &[(String, Vec<u8>)],
) -> Result<(), String> {
    let directory = profiles_root
        .join(profile_id)
        .join("BepInEx")
        .join("plugins")
        .join("LevelImposter");
    match fs::symlink_metadata(&directory) {
        Ok(metadata) if is_reparse(&metadata) || !metadata.is_dir() => {
            return Err("LevelImposter map path is not a regular directory.".into());
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound && downloads.is_empty() => {
            return Ok(());
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
        }
        Err(error) => return Err(error.to_string()),
    }

    let selected = downloads
        .iter()
        .map(|(id, _)| id.to_ascii_lowercase())
        .collect::<HashSet<_>>();
    for id in previously_owned {
        if selected.contains(&id.to_ascii_lowercase()) {
            continue;
        }
        let path = directory.join(format!("{id}.lim2"));
        match fs::symlink_metadata(&path) {
            Ok(metadata) if is_reparse(&metadata) || !metadata.is_file() => {
                return Err("Managed LevelImposter map is not a regular file.".into());
            }
            Ok(_) => fs::remove_file(path).map_err(|error| error.to_string())?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    for (id, bytes) in downloads {
        atomic_write(&directory.join(format!("{id}.lim2")), bytes)?;
    }
    Ok(())
}

fn normalize_levelimposter_map_ids(map_ids: Vec<String>) -> Result<Vec<String>, String> {
    if map_ids.is_empty() || map_ids.len() > MAX_LEVELIMPOSTER_MAPS_PER_BATCH {
        return Err(format!(
            "Select between 1 and {MAX_LEVELIMPOSTER_MAPS_PER_BATCH} maps."
        ));
    }
    let mut seen = HashSet::with_capacity(map_ids.len());
    let mut normalized = Vec::with_capacity(map_ids.len());
    for id in map_ids {
        let id = id.trim().to_ascii_lowercase();
        if !valid_levelimposter_map_id(&id) {
            return Err("LevelImposter map IDs must be UUIDs.".into());
        }
        if !seen.insert(id.clone()) {
            return Err(format!(
                "LevelImposter map {id} was selected more than once."
            ));
        }
        normalized.push(id);
    }
    Ok(normalized)
}

fn install_levelimposter_maps_impl(
    profile_id: String,
    map_ids: Vec<String>,
    reporter: &ProgressReporter,
) -> Result<ProfileRecord, String> {
    reporter.stage(
        "preparing",
        format!(
            "Preparing {} map download{}",
            map_ids.len(),
            if map_ids.len() == 1 { "" } else { "s" }
        ),
    );
    let normalized = normalize_levelimposter_map_ids(map_ids)?;

    let http = ProgressHttp::new(http()?, reporter.clone());
    let downloads = download_levelimposter_maps(&http, &normalized)?;

    let arch = profile_arch(&profile_id)?;
    let (store, runtime) = profile_store_runtime(&profile_id)?;
    let catalog = catalog()?;
    let levelimposter = catalog
        .get(LEVELIMPOSTER_ID)
        .cloned()
        .ok_or("LevelImposter is missing from the trusted catalog.")?;
    let bundled = bundled_catalog();
    let authoritative = bundled
        .get(LEVELIMPOSTER_ID)
        .ok_or("LevelImposter is missing from the bundled trusted catalog.")?;
    let hosted_repo = levelimposter.repo.as_deref().unwrap_or(&levelimposter.id);
    let authoritative_repo = authoritative.repo.as_deref().unwrap_or(&authoritative.id);
    if levelimposter.trust != Trust::Trusted
        || !hosted_repo.eq_ignore_ascii_case(authoritative_repo)
    {
        return Err("LevelImposter catalog metadata is not trusted.".into());
    }
    let plan =
        deps::resolve(&catalog, &[levelimposter.id.clone()]).map_err(|error| error.to_string())?;
    let ordered = &plan.ordered;
    validate_authoritative_dependencies(&catalog, &[levelimposter.id.clone()])?;

    let root = settings::profiles_root();
    profile_transaction(&root, &profile_id, |stage_root, stage_store| {
        let mut record = stage_store
            .load(&profile_id)
            .map_err(|error| error.to_string())?
            .ok_or("profile not found")?;
        let already_installed = record
            .mods
            .iter()
            .any(|installed| installed.package_id.eq_ignore_ascii_case(LEVELIMPOSTER_ID));
        if !already_installed {
            let install = InstallContext {
                stage_root,
                profile_id: &profile_id,
                http: &http,
                catalog: &catalog,
                arch: &arch,
                store,
                runtime,
            };
            for id in ordered {
                install_catalog_latest(
                    &install,
                    &mut record,
                    id,
                    !id.eq_ignore_ascii_case(LEVELIMPOSTER_ID),
                    plan.requirements.get(id).map_or(&[][..], Vec::as_slice),
                )?;
            }
            normalize_dependency_ownership(stage_root, &profile_id, &mut record, &catalog)?;
        }

        reporter.stage("finalizing", "Writing maps and saving the profile");
        replace_profile_levelimposter_maps(stage_root, &profile_id, &[], &downloads)?;
        for (id, _) in &downloads {
            if !record
                .levelimposter_maps
                .iter()
                .any(|installed| installed.eq_ignore_ascii_case(id))
            {
                record.levelimposter_maps.push(id.clone());
            }
        }
        record.levelimposter_maps.sort();
        stage_store
            .save(&record)
            .map_err(|error| error.to_string())?;
        Ok(record)
    })
}

fn remove_levelimposter_maps_impl(
    profile_id: String,
    map_ids: Vec<String>,
) -> Result<ProfileRecord, String> {
    validate_profile_id(&profile_id)?;
    let normalized = normalize_levelimposter_map_ids(map_ids)?;
    let removed = normalized
        .iter()
        .map(|id| id.to_ascii_lowercase())
        .collect::<HashSet<_>>();
    let root = settings::profiles_root();
    profile_transaction(&root, &profile_id, |stage_root, stage_store| {
        let mut record = stage_store
            .load(&profile_id)
            .map_err(|error| error.to_string())?
            .ok_or("profile not found")?;
        replace_profile_levelimposter_maps(stage_root, &profile_id, &normalized, &[])?;
        record
            .levelimposter_maps
            .retain(|id| !removed.contains(&id.to_ascii_lowercase()));
        stage_store
            .save(&record)
            .map_err(|error| error.to_string())?;
        Ok(record)
    })
}
// ---------- mod mutations ----------

#[tauri::command]
pub async fn add_mod(
    profile_id: String,
    repo: String,
    arch: String,
) -> Result<ProfileRecord, String> {
    blocking(move || {
        let _guard = lock_profile_mutation(&profile_id)?;
        require_profile_source_for_install(&profile_id)?;
        add_mod_impl(profile_id, repo, arch)
    })
    .await
}

fn add_mod_impl(profile_id: String, repo: String, arch: String) -> Result<ProfileRecord, String> {
    validate_profile_id(&profile_id)?;
    let _ = arch;
    let arch = profile_arch(&profile_id)?;
    let (store, runtime) = profile_store_runtime(&profile_id)?;
    let repo = resolver::parse_repo(&repo).ok_or("invalid repo or URL")?;
    let catalog = catalog()?;
    let root_entry = catalog_entry_for(&catalog, &repo).cloned();
    let (ordered, requirements) = match root_entry.as_ref() {
        Some(entry) => {
            let plan = deps::resolve(&catalog, std::slice::from_ref(&entry.id))
                .map_err(|error| error.to_string())?;
            (plan.ordered, plan.requirements)
        }
        None => (Vec::new(), HashMap::new()),
    };
    let root = settings::profiles_root();
    profile_transaction(&root, &profile_id, |stage_root, stage_store| {
        let mut record = stage_store
            .load(&profile_id)
            .map_err(|error| error.to_string())?
            .ok_or("profile not found")?;
        let mut explicit_roots = explicit_catalog_roots(&record, &catalog);
        if let Some(entry) = root_entry.as_ref() {
            if !explicit_roots
                .iter()
                .any(|root| root.eq_ignore_ascii_case(&entry.id))
            {
                explicit_roots.push(entry.id.clone());
            }
        }
        validate_authoritative_dependencies(&catalog, &explicit_roots)?;
        let http = http()?;
        let install = InstallContext {
            stage_root,
            profile_id: &profile_id,
            http: &http,
            catalog: &catalog,
            arch: &arch,
            store,
            runtime,
        };
        for id in &ordered {
            let managed = root_entry
                .as_ref()
                .is_some_and(|root| is_managed_dependency(&root.id, id));
            install_catalog_latest(
                &install,
                &mut record,
                id,
                managed,
                requirements.get(id).map_or(&[][..], Vec::as_slice),
            )?;
        }
        if root_entry.is_none() {
            let rules = default_rules();
            let resolved = resolve_profile_latest(&http, &repo, &rules, &arch, store, runtime)?;
            install_record(
                &install,
                &mut record,
                InstallRequest {
                    package_id: repo.clone(),
                    name: repo.clone(),
                    repo: repo.clone(),
                    tags: Vec::new(),
                    managed: false,
                    resolved,
                },
            )?;
        }
        normalize_dependency_ownership(stage_root, &profile_id, &mut record, &catalog)?;
        stage_store
            .save(&record)
            .map_err(|error| error.to_string())?;
        Ok(record)
    })
}

#[tauri::command]
pub async fn set_mod_enabled(
    profile_id: String,
    package_id: String,
    enabled: bool,
) -> Result<ProfileRecord, String> {
    blocking(move || {
        let _guard = lock_profile_mutation(&profile_id)?;
        validate_profile_id(&profile_id)?;
        let root = settings::profiles_root();
        recover_profile_transactions(&root)?;
        let store = ProfileStore::new(&root);
        let catalog = catalog()?;
        let mut record = store
            .load(&profile_id)
            .map_err(|error| error.to_string())?
            .ok_or("profile not found")?;
        let position = validate_mod_toggle(&record, &catalog, &package_id)?;
        let previous = record.mods[position].enabled;
        let file = record.mods[position].file.clone();
        if previous == enabled {
            if let Some(file) = file.as_deref() {
                profile::set_plugin_enabled(&root, &profile_id, file, enabled)
                    .map_err(|error| error.to_string())?;
            }
            return Ok(record);
        }
        let Some(file) = file else {
            record.mods[position].enabled = enabled;
            store.save(&record).map_err(|error| error.to_string())?;
            return Ok(record);
        };
        let _recovery_guard = PROFILE_RECOVERY_LOCK
            .lock()
            .map_err(|_| "profile recovery lock is poisoned".to_string())?;
        let journal = write_mod_toggle_recovery_journal(&root, &profile_id, &file)?;
        if let Err(error) = profile::set_plugin_enabled(&root, &profile_id, &file, enabled) {
            let cleanup = fs::remove_file(&journal).err();
            return Err(match cleanup {
                Some(cleanup) => format!(
                    "{error}; additionally could not remove the unused recovery marker ({cleanup}): {}",
                    journal.display()
                ),
                None => error.to_string(),
            });
        }
        record.mods[position].enabled = enabled;
        if let Err(error) = store.save(&record) {
            let rollback = profile::set_plugin_enabled(&root, &profile_id, &file, previous);
            if rollback.is_ok() {
                return match fs::remove_file(&journal) {
                    Ok(()) => Err(error.to_string()),
                    Err(cleanup) => Err(format!(
                        "{error}; the plugin file was restored, but the recovery marker could not be removed ({cleanup}): {}",
                        journal.display()
                    )),
                };
            }
            return Err(format!(
                "could not save the mod toggle ({error}) or restore the plugin file ({}); recovery evidence was retained at {}",
                rollback.expect_err("failed rollback has an error"),
                journal.display()
            ));
        }
        fs::remove_file(&journal).map_err(|error| {
            format!(
                "mod toggle committed but its recovery marker could not be removed ({error}): {}",
                journal.display()
            )
        })?;
        Ok(record)
    })
    .await
}

#[tauri::command]
pub async fn set_mod_version(
    profile_id: String,
    package_id: String,
    version: String,
    arch: String,
) -> Result<ProfileRecord, String> {
    blocking(move || {
        let _guard = lock_profile_mutation(&profile_id)?;
        require_profile_source_for_install(&profile_id)?;
        set_mod_version_impl(profile_id, package_id, version, arch)
    })
    .await
}

fn set_mod_version_impl(
    profile_id: String,
    package_id: String,
    version: String,
    arch: String,
) -> Result<ProfileRecord, String> {
    validate_profile_id(&profile_id)?;
    let _ = arch;
    let arch = profile_arch(&profile_id)?;
    let (store, runtime) = profile_store_runtime(&profile_id)?;
    let catalog = catalog()?;
    let root = settings::profiles_root();
    profile_transaction(&root, &profile_id, |stage_root, stage_store| {
        let mut record = stage_store
            .load(&profile_id)
            .map_err(|error| error.to_string())?
            .ok_or("profile not found")?;
        let position = record
            .mods
            .iter()
            .position(|installed| installed.package_id == package_id)
            .ok_or("mod not found")?;
        let existing = record.mods[position].clone();
        if existing.managed {
            return Err(
                "Managed dependency versions are selected by their root mods. Change the root mod version instead."
                    .into(),
            );
        }
        let repo = existing
            .repo
            .as_deref()
            .and_then(resolver::parse_repo)
            .or_else(|| resolver::parse_repo(&package_id))
            .ok_or("cannot resolve source")?;
        let rules = catalog_entry_for(&catalog, &package_id)
            .map(|entry| entry.asset_rules.clone())
            .unwrap_or_else(default_rules);
        let http = http()?;
        let install = InstallContext {
            stage_root,
            profile_id: &profile_id,
            http: &http,
            catalog: &catalog,
            arch: &arch,
            store,
            runtime,
        };
        let resolved = resolve_profile_tag(&http, &repo, &version, &rules, &arch, store, runtime)?;
        install_record(
            &install,
            &mut record,
            InstallRequest {
                package_id: existing.package_id,
                name: existing.name,
                repo,
                tags: existing.tags,
                managed: existing.managed,
                resolved,
            },
        )?;
        stage_store
            .save(&record)
            .map_err(|error| error.to_string())?;
        Ok(record)
    })
}

#[tauri::command]
pub async fn remove_mod(profile_id: String, package_id: String) -> Result<ProfileRecord, String> {
    blocking(move || {
        let _guard = lock_profile_mutation(&profile_id)?;
        validate_profile_id(&profile_id)?;
        require_profile_source_for_install(&profile_id)?;
        let root = settings::profiles_root();
        profile_transaction(&root, &profile_id, |stage_root, stage_store| {
            let mut record = stage_store
                .load(&profile_id)
                .map_err(|error| error.to_string())?
                .ok_or("profile not found")?;
            let catalog = catalog()?;
            remove_mod_from_record(stage_root, &profile_id, &mut record, &catalog, &package_id)?;
            stage_store
                .save(&record)
                .map_err(|error| error.to_string())?;
            Ok(record)
        })
    })
    .await
}

#[tauri::command]
pub async fn check_mod_updates(profile_id: String, arch: String) -> Result<ProfileRecord, String> {
    blocking(move || {
        let started = Instant::now();
        let _guard = lock_profile_mutation(&profile_id)?;
        validate_profile_id(&profile_id)?;
        let _ = arch;
        let arch = profile_arch(&profile_id)?;
        let (store, runtime) = profile_store_runtime(&profile_id)?;
        let catalog = catalog()?;
        let root = settings::profiles_root();
        let profile_store = recovered_profile_store(&root)?;
        let mut record = profile_store
            .load(&profile_id)
            .map_err(|error| error.to_string())?
            .ok_or("profile not found")?;
        let http = http()?;
        let provided = provided_dependency_ids(&record, &catalog);
        let mut candidates = Vec::new();
        for (position, installed) in record.mods.iter_mut().enumerate() {
            if provided.contains(&installed.package_id.to_ascii_lowercase())
                || installed
                    .repo
                    .as_deref()
                    .is_some_and(|repo| provided.contains(&repo.to_ascii_lowercase()))
            {
                installed.update = None;
                continue;
            }
            let Some(repo) = installed
                .repo
                .as_deref()
                .and_then(resolver::parse_repo)
                .or_else(|| resolver::parse_repo(&installed.package_id))
            else {
                continue;
            };
            let rules = catalog_entry_for(&catalog, &installed.package_id)
                .map(|entry| entry.asset_rules.clone())
                .unwrap_or_else(default_rules);
            candidates.push((position, repo, rules));
        }

        for chunk in candidates.chunks(4) {
            let results = std::thread::scope(|scope| {
                chunk
                    .iter()
                    .map(|(position, repo, rules)| {
                        let client = http.clone();
                        let repo = repo.clone();
                        let rules = rules.clone();
                        let arch = arch.clone();
                        scope.spawn(move || {
                            (
                                *position,
                                latest_profile_version(
                                    &client, &repo, &rules, &arch, store, runtime,
                                ),
                            )
                        })
                    })
                    .map(|handle| {
                        handle
                            .join()
                            .map_err(|_| "update resolver worker panicked".to_string())
                    })
                    .collect::<Result<Vec<_>, String>>()
            })?;
            for (position, latest) in results {
                let installed = &mut record.mods[position];
                match latest {
                    Ok(latest) => {
                        installed.update =
                            perfect_sync_core::version::is_newer(&latest, &installed.version)
                                .then_some(latest);
                    }
                    Err(error) => {
                        log::warn!(
                            "could not refresh update metadata for {}: {error}",
                            installed.package_id
                        );
                    }
                }
            }
        }
        profile_store
            .save(&record)
            .map_err(|error| error.to_string())?;
        log_perf("update_metadata_resolution", started, candidates.len(), 0);
        Ok(record)
    })
    .await
}

#[tauri::command]
pub async fn apply_mod_updates(
    profile_id: String,
    package_ids: Vec<String>,
    arch: String,
    on_progress: Channel<OperationProgress>,
) -> Result<ProfileRecord, String> {
    blocking(move || {
        let _guard = lock_profile_mutation(&profile_id)?;
        validate_profile_id(&profile_id)?;
        require_profile_source_for_install(&profile_id)?;
        if package_ids.is_empty() || package_ids.len() > 64 {
            return Err("Choose between 1 and 64 reviewed mod updates.".into());
        }
        let mut selected = HashSet::new();
        for package_id in &package_ids {
            if package_id.is_empty() || !selected.insert(package_id.to_ascii_lowercase()) {
                return Err("Reviewed mod updates contain an invalid or duplicate id.".into());
            }
        }
        let _ = arch;
        let arch = profile_arch(&profile_id)?;
        let (store, runtime) = profile_store_runtime(&profile_id)?;
        let catalog = catalog()?;
        let root = settings::profiles_root();
        let reporter = ProgressReporter::new(on_progress);
        profile_transaction(&root, &profile_id, |stage_root, stage_store| {
            let mut record = stage_store
                .load(&profile_id)
                .map_err(|error| error.to_string())?
                .ok_or("profile not found")?;
            let progress_http = ProgressHttp::new(http()?, reporter.clone());
            for (index, package_id) in package_ids.iter().enumerate() {
                let position = record
                    .mods
                    .iter()
                    .position(|installed| installed.package_id.eq_ignore_ascii_case(package_id))
                    .ok_or_else(|| format!("Reviewed mod is no longer installed: {package_id}"))?;
                let existing = record.mods[position].clone();
                if existing.managed {
                    return Err(format!(
                        "{} is an automatic dependency. Update its root mod instead.",
                        existing.name
                    ));
                }
                let version = existing
                    .update
                    .as_deref()
                    .filter(|version| {
                        perfect_sync_core::version::is_newer(version, &existing.version)
                    })
                    .ok_or_else(|| {
                        format!("{} no longer has a newer reviewed version.", existing.name)
                    })?
                    .to_string();
                let repo = existing
                    .repo
                    .as_deref()
                    .and_then(resolver::parse_repo)
                    .or_else(|| resolver::parse_repo(&existing.package_id))
                    .ok_or_else(|| format!("Cannot resolve the source for {}.", existing.name))?;
                let rules = catalog_entry_for(&catalog, &existing.package_id)
                    .map(|entry| entry.asset_rules.clone())
                    .unwrap_or_else(default_rules);
                reporter.stage(
                    "resolving",
                    format!(
                        "Resolving {} {} ({}/{})",
                        existing.name,
                        version,
                        index + 1,
                        package_ids.len()
                    ),
                );
                let resolved = resolve_profile_tag(
                    &progress_http,
                    &repo,
                    &version,
                    &rules,
                    &arch,
                    store,
                    runtime,
                )?;
                let install = InstallContext {
                    stage_root,
                    profile_id: &profile_id,
                    http: &progress_http,
                    catalog: &catalog,
                    arch: &arch,
                    store,
                    runtime,
                };
                install_record(
                    &install,
                    &mut record,
                    InstallRequest {
                        package_id: existing.package_id,
                        name: existing.name,
                        repo,
                        tags: existing.tags,
                        managed: false,
                        resolved,
                    },
                )?;
            }
            normalize_dependency_ownership(stage_root, &profile_id, &mut record, &catalog)?;
            reporter.stage("finalizing", "Saving the reviewed mod update batch");
            stage_store
                .save(&record)
                .map_err(|error| error.to_string())?;
            Ok(record)
        })
    })
    .await
}

#[tauri::command]
pub async fn apply_lobby_code(
    code: String,
    arch: String,
    game_instance_id: Option<String>,
    on_progress: Channel<OperationProgress>,
) -> Result<ProfileRecord, String> {
    blocking(move || {
        let _guard = lock_mutations()?;
        let _profile_guards = lock_all_profile_mutations()?;
        require_instance_source_for_install(game_instance_id.as_deref())?;
        let reporter = ProgressReporter::new(on_progress);
        apply_lobby_code_impl(code, arch, game_instance_id, &reporter)
    })
    .await
}

fn lobby_digest(code: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = Sha256::digest(code.as_bytes());
    let mut encoded = String::with_capacity(64);
    for byte in digest {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

fn apply_lobby_code_impl(
    code: String,
    arch: String,
    game_instance_id: Option<String>,
    reporter: &ProgressReporter,
) -> Result<ProfileRecord, String> {
    reporter.stage("preparing", "Reading and validating the lobby code");
    let manifest = codec::decode(&code).map_err(|error| error.to_string())?;
    reporter.stage(
        "resolving",
        format!(
            "Preparing {} mod{} and {} map{}",
            manifest.mods.len(),
            if manifest.mods.len() == 1 { "" } else { "s" },
            manifest.levelimposter_maps.len(),
            if manifest.levelimposter_maps.len() == 1 {
                ""
            } else {
                "s"
            },
        ),
    );
    let settings = settings::load().map_err(|error| error.to_string())?;
    let instance_id = game_instance_id
        .as_deref()
        .ok_or("choose an Among Us instance before applying a lobby")?;
    let target_instance = settings
        .game_instances
        .iter()
        .find(|instance| instance.id == instance_id)
        .ok_or("lobby profile refers to an unknown game instance")?;
    let _ = arch;
    let arch = saved_game_arch(Some(instance_id))?;
    let store = target_instance.store;
    let runtime = target_instance.runtime;
    let display = manifest
        .name
        .clone()
        .unwrap_or_else(|| "Imported lobby".to_string());
    let slug: String = display
        .to_ascii_lowercase()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '-'
            }
        })
        .take(16)
        .collect();
    let digest = lobby_digest(&code);
    let slug = slug.trim_matches('-');
    let id = if slug.is_empty() {
        format!("lobby-{}", &digest[..40])
    } else {
        format!("lobby-{slug}-{}", &digest[..40])
    };
    validate_profile_id(&id)?;
    let catalog = catalog()?;
    let selected: Vec<String> = manifest
        .mods
        .iter()
        .filter_map(|manifest_mod| {
            catalog_entry_for(&catalog, &manifest_mod.id).map(|entry| entry.id.clone())
        })
        .collect();
    let selected_provided_ids = catalog_provided_dependency_ids(&catalog, &selected);
    validate_authoritative_dependencies(&catalog, &selected)?;
    let included_dependencies = selected_dependencies(&catalog, &selected)?;
    let http = ProgressHttp::new(http()?, reporter.clone());
    let root = settings::profiles_root();
    profile_transaction(&root, &id, |stage_root, stage_store| {
        let marker = stage_store
            .profile_dir(&id)
            .map_err(|error| error.to_string())?
            .join(".perfectsync-lobby-source");
        let existing_record = stage_store.load(&id).map_err(|error| error.to_string())?;
        match (existing_record.is_some(), read_bounded(&marker, 128)?) {
            (_, Some(existing)) if existing != digest.as_bytes() => {
                return Err("lobby profile id belongs to a different lobby source".into());
            }
            (true, None) => {
                return Err("refusing to overwrite a profile with unknown lobby source".into());
            }
            _ => {}
        }
        let map_downloads = download_levelimposter_maps(&http, &manifest.levelimposter_maps)?;
        let mut record = existing_record.unwrap_or(ProfileRecord {
            id: id.clone(),
            name: display.clone(),
            crew_color: "#ffd23f".to_string(),
            game_build: None,
            game_instance_id: game_instance_id.clone(),
            mods: Vec::new(),
            levelimposter_maps: Vec::new(),
        });
        let previously_owned_maps = record.levelimposter_maps.clone();
        if profile_has_tou_mira(&record) {
            profile::remove_tou_bundle(stage_root, &id).map_err(|error| error.to_string())?;
        }
        for old in &record.mods {
            if let Some(file) = old.file.as_deref() {
                profile::remove_plugin(stage_root, &id, file).map_err(|error| error.to_string())?;
            }
        }
        record.mods.clear();
        record.name = display.clone();
        record.game_build = None;
        record.game_instance_id = game_instance_id.clone();
        let install = InstallContext {
            stage_root,
            profile_id: &id,
            http: &http,
            catalog: &catalog,
            arch: &arch,
            store,
            runtime,
        };
        for manifest_mod in &manifest.mods {
            let entry = catalog_entry_for(&catalog, &manifest_mod.id);
            if entry
                .is_some_and(|entry| selected_provided_ids.contains(&entry.id.to_ascii_lowercase()))
            {
                continue;
            }
            let repo = entry
                .and_then(|entry| entry.repo.clone())
                .or_else(|| resolver::parse_repo(&manifest_mod.id))
                .ok_or_else(|| format!("cannot resolve source for {}", manifest_mod.id))?;
            let rules = entry
                .map(|entry| entry.asset_rules.clone())
                .unwrap_or_else(default_rules);
            let resolved = if let Some(asset) = manifest_mod.asset.as_deref() {
                selected_release_asset(
                    &http,
                    &repo,
                    &manifest_mod.v,
                    asset,
                    ReleaseAssetTarget {
                        rules: Some(&rules),
                        arch: &arch,
                        store,
                        runtime,
                    },
                )?
            } else {
                resolve_profile_tag(&http, &repo, &manifest_mod.v, &rules, &arch, store, runtime)?
            };
            install_record(
                &install,
                &mut record,
                InstallRequest {
                    package_id: entry
                        .map(|entry| entry.id.clone())
                        .unwrap_or_else(|| manifest_mod.id.clone()),
                    name: entry
                        .map(|entry| entry.name.clone())
                        .unwrap_or_else(|| repo.clone()),
                    repo,
                    tags: entry.map(|entry| entry.tags.clone()).unwrap_or_default(),
                    managed: entry.is_some_and(|entry| {
                        included_dependencies.contains(&entry.id.to_ascii_lowercase())
                    }),
                    resolved,
                },
            )?;
        }
        for personal in settings
            .personal_mods
            .iter()
            .filter(|personal| personal.enabled)
        {
            let repo = resolver::parse_repo(&personal.repo)
                .ok_or_else(|| format!("invalid personal mod source {}", personal.repo))?;
            if let Some(installed) = record.mods.iter_mut().find(|installed| {
                installed.package_id.eq_ignore_ascii_case(&repo)
                    || installed
                        .repo
                        .as_deref()
                        .is_some_and(|source| source.eq_ignore_ascii_case(&repo))
            }) {
                installed.managed = false;
                continue;
            }
            let entry = catalog_entry_for(&catalog, &repo);
            let resolved = selected_release_asset(
                &http,
                &repo,
                &personal.tag,
                &personal.asset,
                ReleaseAssetTarget {
                    rules: entry.map(|entry| &entry.asset_rules),
                    arch: &arch,
                    store,
                    runtime,
                },
            )?;
            install_record(
                &install,
                &mut record,
                InstallRequest {
                    package_id: repo.clone(),
                    name: personal
                        .name
                        .clone()
                        .or_else(|| entry.map(|entry| entry.name.clone()))
                        .unwrap_or_else(|| repo.clone()),
                    repo,
                    tags: entry.map(|entry| entry.tags.clone()).unwrap_or_default(),
                    managed: false,
                    resolved,
                },
            )?;
        }
        for local in settings
            .personal_local_mods
            .iter()
            .filter(|local| local.enabled)
        {
            install_local_mod_into_record(stage_root, &id, &mut record, Path::new(&local.path))?;
        }
        reporter.stage(
            "finalizing",
            "Verifying files and publishing the lobby profile",
        );
        replace_profile_levelimposter_maps(
            stage_root,
            &id,
            &previously_owned_maps,
            &map_downloads,
        )?;
        record.levelimposter_maps = map_downloads
            .iter()
            .map(|(map_id, _)| map_id.clone())
            .collect();
        normalize_dependency_ownership(stage_root, &id, &mut record, &catalog)?;
        stage_store
            .save(&record)
            .map_err(|error| error.to_string())?;
        atomic_write(&marker, digest.as_bytes())?;
        Ok(record)
    })
}

// ---------- loader + launch ----------

#[tauri::command]
pub async fn list_unmanaged_plugins(
    game_path: String,
    profile_id: String,
) -> Result<Vec<loader::UnmanagedPlugin>, String> {
    blocking(move || {
        let _guard = lock_mutations()?;
        validate_profile_id(&profile_id)?;
        if managed_instance::active_marker(&profile_id)?.is_some() {
            return Ok(Vec::new());
        }
        let game_dir = validate_game_target(&game_path, Some(&profile_id))?;
        with_existing_profile_layout(&settings::profiles_root(), &profile_id, || {
            loader::unmanaged_plugins(&settings::profiles_root(), &profile_id, &game_dir)
                .map_err(|error| error.to_string())
        })
    })
    .await
}

#[tauri::command]
pub async fn quarantine_unmanaged_plugins(
    game_path: String,
    profile_id: String,
    paths: Vec<String>,
) -> Result<Vec<loader::UnmanagedPlugin>, String> {
    blocking(move || {
        let _guard = lock_mutations()?;
        validate_profile_id(&profile_id)?;
        let game_dir = validate_game_target(&game_path, Some(&profile_id))?;
        game_path_is_stopped(&game_dir)?;
        with_existing_profile_layout(&settings::profiles_root(), &profile_id, || {
            game_artifact_transaction(&game_dir, || {
                loader::quarantine_unmanaged_plugins(
                    &settings::profiles_root(),
                    &profile_id,
                    &game_dir,
                    &paths,
                )
                .map_err(|error| error.to_string())
            })
        })
    })
    .await
}

#[tauri::command]
pub async fn delete_unmanaged_plugins(
    game_path: String,
    profile_id: String,
    paths: Vec<String>,
) -> Result<Vec<loader::UnmanagedPlugin>, String> {
    blocking(move || {
        let _guard = lock_mutations()?;
        validate_profile_id(&profile_id)?;
        let game_dir = validate_game_target(&game_path, Some(&profile_id))?;
        game_path_is_stopped(&game_dir)?;
        with_existing_profile_layout(&settings::profiles_root(), &profile_id, || {
            game_artifact_transaction(&game_dir, || {
                loader::delete_unmanaged_plugins(
                    &settings::profiles_root(),
                    &profile_id,
                    &game_dir,
                    &paths,
                )
                .map_err(|error| error.to_string())
            })
        })
    })
    .await
}

#[tauri::command]
pub async fn import_unmanaged_plugins(
    game_path: String,
    profile_id: String,
    paths: Vec<String>,
) -> Result<ProfileRecord, String> {
    blocking(move || {
        let _guard = lock_mutations()?;
        validate_profile_id(&profile_id)?;
        require_profile_source_for_install(&profile_id)?;
        let game_dir = validate_game_target(&game_path, Some(&profile_id))?;
        let profiles_root = settings::profiles_root();
        let plugins =
            loader::selected_unmanaged_plugins(&profiles_root, &profile_id, &game_dir, &paths)
                .map_err(|error| error.to_string())?;
        if plugins.iter().any(|plugin| !plugin.importable) {
            return Err(
                "Plugins inside subfolders cannot be imported safely. Quarantine them, then add their supported releases to the profile."
                    .into(),
            );
        }
        profile_transaction(&profiles_root, &profile_id, |stage_root, stage_store| {
            let mut record = stage_store
                .load(&profile_id)
                .map_err(|error| error.to_string())?
                .ok_or("profile not found")?;
            for plugin in &plugins {
                let source = game_dir
                    .join("BepInEx")
                    .join("plugins")
                    .join(&plugin.path);
                install_local_mod_into_record(
                    stage_root,
                    &profile_id,
                    &mut record,
                    &source,
                )?;
            }
            stage_store
                .save(&record)
                .map_err(|error| error.to_string())?;
            Ok(record)
        })
    })
    .await
}

// ---------- loader + launch ----------

#[tauri::command]
pub async fn ensure_loader(
    game_path: String,
    profile_id: String,
    arch: String,
    apply_doorstop_fix: bool,
    on_progress: Channel<OperationProgress>,
) -> Result<Option<String>, String> {
    blocking(move || {
        let _guard = lock_profile_mutation(&profile_id)?;
        workspace_is_stopped(&profile_id)?;
        let reporter = ProgressReporter::new(on_progress);
        let profiles_root = settings::profiles_root();
        let profile_store = recovered_profile_store(&profiles_root)?;
        let profile = profile_store
            .load(&profile_id)
            .map_err(|error| error.to_string())?
            .ok_or("profile not found")?;
        let instance = profile_game_instance(&game_path, &profile)?;
        if arch_str(instance.arch) != arch {
            return Err("requested loader architecture does not match the game source".into());
        }
        require_profile_source_for_install(&profile_id)?;
        if profile_uses_tou_mira(&profile) && apply_doorstop_fix {
            return Err(
                "Town of Us includes its own UnityDoorstop build; the separate compatibility fix cannot be applied."
                    .into(),
            );
        }
        let profile_root = profile_store
            .profile_dir(&profile_id)
            .map_err(|error| error.to_string())?;
        managed_instance::save_loader_preference(
            &profile_root,
            apply_doorstop_fix,
            Some(PINNED_LOADER_VERSION.to_string()),
        )?;
        prepare_profile(&game_path, &profile_id, Some(&reporter), false)
    })
    .await
}

#[tauri::command]
pub async fn reinstall_loader(
    game_path: String,
    profile_id: String,
    arch: String,
    apply_doorstop_fix: bool,
    use_latest_loader: bool,
) -> Result<Option<String>, String> {
    blocking(move || {
        let _guard = lock_profile_mutation(&profile_id)?;
        workspace_is_stopped(&profile_id)?;
        let profiles_root = settings::profiles_root();
        let profile_store = recovered_profile_store(&profiles_root)?;
        let profile = profile_store
            .load(&profile_id)
            .map_err(|error| error.to_string())?
            .ok_or("profile not found")?;
        let instance = profile_game_instance(&game_path, &profile)?;
        if arch_str(instance.arch) != arch {
            return Err("requested loader architecture does not match the game source".into());
        }
        require_profile_source_for_install(&profile_id)?;
        if profile_uses_tou_mira(&profile) {
            return Err(
                "Town of Us owns this profile's BepInEx build. Reinstall or change the Town of Us release instead."
                    .into(),
            );
        }
        let http = http()?;
        let (version, url) = if use_latest_loader {
            resolve_loader(&http, &arch)?
        } else {
            pinned_loader(&arch)?
        };
        let bytes = http.get_bytes(&url).map_err(|error| error.to_string())?;
        let cache = loader::loader_cache_dir(&settings::cache_dir(), &version, &arch)
            .map_err(|error| error.to_string())?;
        loader::publish_pack_cache(&bytes, &cache).map_err(|error| error.to_string())?;
        let profile_root = profile_store
            .profile_dir(&profile_id)
            .map_err(|error| error.to_string())?;
        managed_instance::save_loader_preference(
            &profile_root,
            apply_doorstop_fix,
            Some(version),
        )?;
        prepare_profile(&game_path, &profile_id, None, true)
    })
    .await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoaderStatus {
    pub game_found: bool,
    pub winhttp: bool,
    pub preloader: bool,
    pub current: bool,
    pub installed_version: Option<String>,
    pub doorstop_fix: bool,
    pub dotnet: bool,
    pub steam_appid: bool,
    pub profile_plugins: usize,
    pub game_plugins: usize,
    pub runtime: Runtime,
    pub runtime_ready: bool,
    pub workspace_ready: bool,
    pub workspace_path: Option<String>,
}

#[tauri::command]
pub async fn loader_status(game_path: String, profile_id: String) -> Result<LoaderStatus, String> {
    blocking(move || {
        let _guard = lock_profile_mutation(&profile_id)?;
        validate_profile_id(&profile_id)?;
        let source = validate_game_target(&game_path, Some(&profile_id))?;
        let root = settings::profiles_root();
        let profile_store = recovered_profile_store(&root)?;
        let profile = profile_store
            .load(&profile_id)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "profile not found".to_string())?;
        let instance = profile_game_instance(&game_path, &profile)?;
        let profile_root = profile_store
            .profile_dir(&profile_id)
            .map_err(|error| error.to_string())?;
        let profile_plugins = profile_root.join("BepInEx").join("plugins");
        let count_dll = |directory: PathBuf| -> Result<usize, String> {
            let entries = match fs::read_dir(directory) {
                Ok(entries) => entries,
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
                Err(error) => return Err(error.to_string()),
            };
            let mut count = 0;
            for entry in entries {
                let entry = entry.map_err(|error| error.to_string())?;
                let metadata = entry.metadata().map_err(|error| error.to_string())?;
                if metadata.is_file()
                    && entry
                        .path()
                        .extension()
                        .and_then(|extension| extension.to_str())
                        .is_some_and(|extension| extension.eq_ignore_ascii_case("dll"))
                {
                    count += 1;
                }
            }
            Ok(count)
        };
        let context = compat::resolve_with_hint(&source, Some(instance.runtime));
        let runtime_ready = context.runtime == Runtime::Native
            || context
                .prefix
                .as_deref()
                .is_some_and(compat::has_winhttp_override);
        let arch = game::exe_arch(&source.join(process::GAME_EXE))
            .map(arch_str)
            .ok_or("Among Us executable architecture is unsupported")?;
        let revision = managed_instance::profile_revision(&profile_root)?;
        let workspace = managed_instance::workspace_game_dir(&profile_id)?;
        let workspace_ready = managed_instance::active_marker(&profile_id)?.is_some_and(|marker| {
            marker.game_instance_id == instance.id
                && marker.profile_id == profile_id
                && marker.profile_revision == revision
        });
        let inspected = workspace_ready.then_some(workspace.as_path());
        Ok(LoaderStatus {
            game_found: true,
            winhttp: inspected.is_some_and(|game| game.join("winhttp.dll").is_file()),
            preloader: inspected.is_some_and(|game| {
                game.join("BepInEx")
                    .join("core")
                    .join(loader::IL2CPP_PRELOADER)
                    .is_file()
            }),
            current: inspected.is_some_and(loader::has_loader),
            installed_version: inspected.and_then(loader::installed_version),
            doorstop_fix: inspected.is_some_and(|game| doorstop_fix_is_current(game, &arch)),
            dotnet: inspected.is_some_and(|game| game.join("dotnet").join("coreclr.dll").is_file()),
            steam_appid: inspected.is_some_and(|game| game.join("steam_appid.txt").is_file()),
            profile_plugins: count_dll(profile_plugins)?,
            game_plugins: inspected
                .map(|game| count_dll(game.join("BepInEx").join("plugins")))
                .transpose()?
                .unwrap_or(0),
            runtime: context.runtime,
            runtime_ready,
            workspace_ready,
            workspace_path: workspace_ready.then(|| workspace.to_string_lossy().into_owned()),
        })
    })
    .await
}

fn load_tou_package_bytes(
    installed: &InstalledMod,
    arch: &str,
    store: Store,
    runtime: Runtime,
) -> Result<Vec<u8>, String> {
    let asset_name = installed
        .asset
        .as_deref()
        .ok_or("Town of Us profile is missing its exact release asset")?;
    let cache_path = tou_package_cache_path(&installed.version, asset_name);
    if let Some(bytes) = read_bounded(&cache_path, MAX_TOU_PACKAGE_CACHE_BYTES)? {
        return Ok(bytes);
    }
    let repo = installed
        .repo
        .as_deref()
        .and_then(resolver::parse_repo)
        .or_else(|| resolver::parse_repo(&installed.package_id))
        .ok_or("Town of Us profile has no valid release source")?;
    let catalog = catalog()?;
    let rules = catalog_entry_for(&catalog, &repo)
        .map(|entry| entry.asset_rules.clone())
        .ok_or("Town of Us is missing its authoritative catalog rules")?;
    let http = http()?;
    let resolved = selected_release_asset(
        &http,
        &repo,
        &installed.version,
        asset_name,
        ReleaseAssetTarget {
            rules: Some(&rules),
            arch,
            store,
            runtime,
        },
    )?;
    if !resolved.asset_name.eq_ignore_ascii_case(asset_name) {
        return Err(format!(
            "Town of Us {} no longer resolves to the profile's exact asset {}",
            installed.version, asset_name
        ));
    }
    let bytes = download_resolved(&http, &resolved).map_err(|error| error.to_string())?;
    cache_tou_package(&installed.version, asset_name, &bytes)?;
    Ok(bytes)
}

fn saved_profile_instance(
    game_path: &str,
    profile: &ProfileRecord,
) -> Result<settings::GameInstance, String> {
    let saved = settings::load().map_err(|error| error.to_string())?;
    let instance = selected_profile_instance(&saved, profile.game_instance_id.as_deref())?;
    let normalize = |path: &str| {
        path.replace('\\', "/")
            .trim_end_matches('/')
            .to_ascii_lowercase()
    };
    if normalize(&instance.path) != normalize(game_path) {
        return Err("game folder does not match the profile's saved source".into());
    }
    Ok(instance.clone())
}

fn profile_game_instance(
    game_path: &str,
    profile: &ProfileRecord,
) -> Result<settings::GameInstance, String> {
    let canonical = validate_game_target(game_path, Some(&profile.id))?;
    let saved = settings::load().map_err(|error| error.to_string())?;
    let instance = selected_profile_instance(&saved, profile.game_instance_id.as_deref())?;
    if !same_path(Path::new(&instance.path), &canonical) {
        return Err("game folder does not match the profile's saved source".into());
    }
    Ok(instance.clone())
}

fn saved_registered_game_instance(game_path: &str) -> Result<settings::GameInstance, String> {
    let normalize = |path: &str| {
        path.replace('\\', "/")
            .trim_end_matches('/')
            .to_ascii_lowercase()
    };
    settings::load()
        .map_err(|error| error.to_string())?
        .game_instances
        .into_iter()
        .find(|instance| normalize(&instance.path) == normalize(game_path))
        .ok_or_else(|| "game source is not registered".to_string())
}

fn cached_loader_pack(
    arch: &str,
    preference: &managed_instance::LoaderPreference,
) -> Result<(PathBuf, String), String> {
    let version = preference
        .loader_version
        .clone()
        .unwrap_or_else(|| PINNED_LOADER_VERSION.to_string());
    let cache = loader::loader_cache_dir(&settings::cache_dir(), &version, arch)
        .map_err(|error| error.to_string())?;
    if let Some(root) = loader::locate_pack_root(&cache) {
        return Ok((root, version));
    }
    if version != PINNED_LOADER_VERSION {
        return Err(format!(
            "The cached experimental BepInEx build {version} is unavailable. Reinstall it or use the pinned build."
        ));
    }
    let (_, url) = pinned_loader(arch)?;
    let bytes = http()?.get_bytes(&url).map_err(|error| error.to_string())?;
    let root = loader::publish_pack_cache(&bytes, &cache).map_err(|error| error.to_string())?;
    Ok((root, version))
}

fn prepare_profile(
    game_path: &str,
    profile_id: &str,
    reporter: Option<&ProgressReporter>,
    force_rebuild: bool,
) -> Result<Option<String>, String> {
    prepare_profile_with_guard(game_path, profile_id, reporter, force_rebuild, || {
        workspace_is_stopped(profile_id)
    })
}

fn prepare_profile_with_guard(
    game_path: &str,
    profile_id: &str,
    reporter: Option<&ProgressReporter>,
    force_rebuild: bool,
    process_guard: impl Fn() -> Result<(), String>,
) -> Result<Option<String>, String> {
    let started = Instant::now();
    process_guard()?;
    let profiles_root = settings::profiles_root();
    let profile_store = recovered_profile_store(&profiles_root)?;
    let profile = profile_store
        .load(profile_id)
        .map_err(|error| error.to_string())?
        .ok_or("profile not found")?;
    managed_instance::migrate_direct_source_storage()?;
    let instance = saved_profile_instance(game_path, &profile)?;
    let source_arch = arch_str(instance.arch).to_string();

    if let Some(reporter) = reporter {
        reporter.stage(
            "preparing",
            "Creating or validating the isolated profile workspace",
        );
    }
    let profile_root = profile_store
        .profile_dir(profile_id)
        .map_err(|error| error.to_string())?;
    with_profile_layout(&profiles_root, profile_id, || Ok(()))?;
    let (previous_revision, previous_material_revision) =
        managed_instance::profile_revisions(&profile_root)?;
    managed_instance::capture_workspace_config(&profiles_root, profile_id)?;
    let (revision, material_revision) = managed_instance::profile_revisions(&profile_root)?;
    if !force_rebuild
        && previous_revision != revision
        && previous_material_revision == material_revision
    {
        managed_instance::refresh_active_profile_revision(
            profile_id,
            profile_id,
            &previous_revision,
            &revision,
        )?;
    }
    if !force_rebuild
        && managed_instance::cached_active_source(
            &instance,
            profile.game_build.as_deref(),
            profile_id,
            &revision,
            &material_revision,
            profile_id,
        )?
        .is_some()
    {
        log_perf(
            "prepare_profile_cached_source",
            started,
            profile.mods.len(),
            0,
        );
        let warning = fs::canonicalize(&instance.path).ok().and_then(|source| {
            let context = compat::resolve_with_hint(&source, Some(instance.runtime));
            configure_runtime_override(&context).err()
        });
        return Ok(warning);
    }
    let source_record =
        managed_instance::source_for_rebuild(&instance, profile.game_build.as_deref())?;
    managed_instance::ensure_source_build_allows_launch(&source_record)?;

    let active_tou = profile
        .mods
        .iter()
        .find(|installed| installed.enabled && is_tou_mira(&installed.package_id));
    let tou_key = active_tou
        .map(|installed| {
            installed
                .asset
                .as_deref()
                .map(|asset| tou_package_key(&installed.version, asset))
                .ok_or("Town of Us profile is missing its exact release asset")
        })
        .transpose()?;
    let shadowed_plugin_files = tou_shadowed_plugin_files(&profile);
    let preference = managed_instance::loader_preference(&profile_root)?;
    if !force_rebuild {
        if let Some(marker) = managed_instance::active_marker(profile_id)? {
            let active = managed_instance::workspace_game_dir(profile_id)?;
            let prior_workspace_is_valid = managed_instance::active_matches(
                &source_record,
                profile_id,
                &marker.profile_revision,
                &marker.material_revision,
                profile_id,
            )?;
            let maps_are_current =
                loader::levelimposter_maps_are_current(&active, &profile.levelimposter_maps)
                    .unwrap_or(false);
            let loader_is_current = if let Some(key) = tou_key.as_deref() {
                loader::tou_package_is_current(&active, key).unwrap_or(false)
            } else {
                let desired_version = preference
                    .loader_version
                    .as_deref()
                    .unwrap_or(PINNED_LOADER_VERSION);
                let doorstop_is_current = if preference.apply_doorstop_fix {
                    doorstop_fix_is_current(&active, &source_arch)
                } else {
                    !loader::has_doorstop_patch_marker(&active)
                };
                matches!(loader::managed_tou_package_key(&active), Ok(None))
                    && loader::has_loader(&active)
                    && loader::installed_version(&active).as_deref() == Some(desired_version)
                    && doorstop_is_current
            };
            if prior_workspace_is_valid && maps_are_current && loader_is_current {
                if let Some(reporter) = reporter {
                    reporter.stage(
                        "preparing",
                        format!("Updating the active {} workspace", profile.name),
                    );
                }
                process_guard()?;
                let delta_result = (|| {
                    loader::sync_profile_plugins_shadowing(
                        &profiles_root,
                        profile_id,
                        &active,
                        &shadowed_plugin_files,
                    )
                    .map_err(|error| error.to_string())?;
                    loader::ensure_steam_appid(&active).map_err(|error| error.to_string())?;
                    loader::write_console_off(&active).map_err(|error| error.to_string())?;
                    process_guard()?;
                    managed_instance::refresh_workspace_marker(
                        &source_record,
                        profile_id,
                        &revision,
                        &material_revision,
                        profile_id,
                    )?;
                    let warning = fs::canonicalize(&instance.path).ok().and_then(|source| {
                        let context = compat::resolve_with_hint(&source, Some(instance.runtime));
                        configure_runtime_override(&context).err()
                    });
                    Ok::<Option<String>, String>(warning)
                })();
                match delta_result {
                    Ok(warning) => {
                        log_perf("prepare_profile_delta", started, profile.mods.len(), 0);
                        return Ok(warning);
                    }
                    Err(error) => {
                        log::warn!(
                            "active workspace delta failed; falling back to a full rebuild: {error}"
                        );
                    }
                }
            }
        }
    }
    let tou_package = active_tou
        .map(|installed| {
            load_tou_package_bytes(installed, &source_arch, instance.store, instance.runtime)
        })
        .transpose()?;
    let loader_pack = if active_tou.is_none() {
        Some(cached_loader_pack(&source_arch, &preference)?)
    } else {
        None
    };
    let doorstop_fix = if active_tou.is_none() && preference.apply_doorstop_fix {
        Some(download_doorstop_fix(&http()?)?)
    } else {
        None
    };

    if let Some(reporter) = reporter {
        reporter.stage(
            "preparing",
            format!("Building an isolated {} workspace", profile.name),
        );
    }
    process_guard()?;
    let stage = managed_instance::begin_workspace(&source_record, profile_id)?;
    let result = (|| {
        if let Some(bytes) = tou_package.as_deref() {
            loader::install_tou_package(
                bytes,
                &stage,
                tou_key
                    .as_deref()
                    .ok_or("Town of Us package key is missing")?,
                PINNED_LOADER_VERSION,
            )
            .map_err(|error| error.to_string())?;
        } else {
            let (pack_root, version) = loader_pack
                .as_ref()
                .ok_or("BepInEx loader package is missing")?;
            loader::install_pack(pack_root, &stage, version).map_err(|error| error.to_string())?;
            if let Some(bytes) = doorstop_fix.as_deref() {
                loader::install_windows_doorstop_patch(
                    bytes,
                    &stage,
                    DOORSTOP_FIX_VERSION,
                    &source_arch,
                )
                .map_err(|error| error.to_string())?;
            }
        }
        loader::sync_profile_plugins_shadowing(
            &profiles_root,
            profile_id,
            &stage,
            &shadowed_plugin_files,
        )
        .map_err(|error| error.to_string())?;
        loader::sync_levelimposter_maps(&profiles_root, profile_id, &stage)
            .map_err(|error| error.to_string())?;
        managed_instance::overlay_profile_config(&profile_root, &stage)?;
        loader::ensure_steam_appid(&stage).map_err(|error| error.to_string())?;
        loader::write_console_off(&stage).map_err(|error| error.to_string())?;
        process_guard()?;
        if let Some(reporter) = reporter {
            reporter.stage(
                "finalizing",
                "Verifying and publishing the isolated workspace",
            );
        }
        managed_instance::publish_workspace(
            &stage,
            &source_record,
            profile_id,
            &revision,
            &material_revision,
            profile_id,
        )?;
        let context = compat::resolve_with_hint(&source_record.source_dir, Some(instance.runtime));
        Ok(configure_runtime_override(&context).err())
    })();
    if result.is_err() && stage.exists() {
        let _ = managed_instance::discard_workspace(&stage, profile_id);
    }
    if result.is_ok() {
        log_perf(
            "prepare_profile_rebuild",
            started,
            source_record.record.file_count as usize,
            source_record.record.byte_count,
        );
    }
    result
}

fn prepare_vanilla(
    game_path: &str,
    workspace_id: &str,
) -> Result<(PathBuf, settings::GameInstance), String> {
    workspace_is_stopped(workspace_id)?;
    managed_instance::migrate_direct_source_storage()?;
    let instance = saved_registered_game_instance(game_path)?;
    managed_instance::capture_workspace_config(&settings::profiles_root(), workspace_id)?;
    let source = managed_instance::source_for_rebuild(&instance, None)?;
    let revision = sha256_hex(format!("vanilla\\0{}", source.record.fingerprint).as_bytes());
    if managed_instance::active_matches(&source, "_vanilla", &revision, &revision, workspace_id)? {
        managed_instance::ensure_source_build_allows_launch(&source)?;
    } else {
        let stage = managed_instance::begin_workspace(&source, workspace_id)?;
        let result = managed_instance::publish_workspace(
            &stage,
            &source,
            "_vanilla",
            &revision,
            &revision,
            workspace_id,
        );
        if result.is_err() && stage.exists() {
            let _ = managed_instance::discard_workspace(&stage, workspace_id);
        }
        result?;
    }
    Ok((
        managed_instance::workspace_game_dir(workspace_id)?,
        instance,
    ))
}

#[tauri::command]
pub async fn sync_profile(
    game_path: String,
    profile_id: String,
    on_progress: Channel<OperationProgress>,
) -> Result<Option<String>, String> {
    blocking(move || {
        let _guard = lock_profile_mutation(&profile_id)?;
        let reporter = ProgressReporter::new(on_progress);
        prepare_profile(&game_path, &profile_id, Some(&reporter), false)
    })
    .await
}

const EPIC_STARTER_URL: &str =
    "https://github.com/whichtwix/EpicGamesStarter/releases/download/1.1.0/EpicGamesStarter.exe.zip";
const EPIC_STARTER_SIZE: u64 = 6_865_606;
const EPIC_STARTER_SHA256: &str =
    "15fb526b39b90a6e571397ec9981faded67e140a6dd3f42c011c7f4060188bc8";
const MAX_EPIC_STARTER_BYTES: u64 = 64 * 1024 * 1024;
const EPIC_EXECUTABLE_SIZE: u64 = 15_316_174;
const EPIC_EXECUTABLE_SHA256: &str =
    "7e1d7e1d2d96aca2ae3a4229f0c2902b1997d94f166f70ce2acd4e7b5bcb8c42";

fn pinned_epic_download() -> Result<ResolvedDownload, String> {
    let release = resolver::parse_release(&format!(
        r#"{{"tag_name":"1.1.0","assets":[{{"name":"EpicGamesStarter.exe.zip","browser_download_url":"{EPIC_STARTER_URL}","size":{EPIC_STARTER_SIZE},"digest":"sha256:{EPIC_STARTER_SHA256}"}}]}}"#
    ))
    .map_err(|error| error.to_string())?;
    let asset = release
        .assets
        .into_iter()
        .next()
        .ok_or("missing Epic pin")?;
    Ok(ResolvedDownload {
        url: asset.url,
        asset_name: asset.name,
        version: release.tag,
        size: asset.size,
    })
}

fn validated_epic_executable(archive: &[u8]) -> Result<Vec<u8>, String> {
    let mut zip = zip::ZipArchive::new(Cursor::new(archive))
        .map_err(|error| format!("invalid EpicGamesStarter ZIP: {error}"))?;
    if zip.len() != 1 {
        return Err("EpicGamesStarter ZIP must contain exactly one entry".into());
    }
    let mut entry = zip
        .by_index(0)
        .map_err(|error| format!("invalid EpicGamesStarter ZIP entry: {error}"))?;
    if entry.is_dir()
        || entry.name() != "EpicGamesStarter.exe"
        || entry.size() == 0
        || entry.size() > MAX_EPIC_STARTER_BYTES
        || entry
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 != 0o100000)
    {
        return Err(
            "EpicGamesStarter ZIP must contain one nonempty regular EpicGamesStarter.exe".into(),
        );
    }
    let expected = entry.size();
    let mut bytes = Vec::with_capacity(expected as usize);
    entry
        .by_ref()
        .take(MAX_EPIC_STARTER_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 != expected {
        return Err("EpicGamesStarter expanded size did not match ZIP metadata".into());
    }
    Ok(bytes)
}

fn sha256_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = Sha256::digest(bytes);
    let mut encoded = String::with_capacity(64);
    for byte in digest {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

fn epic_executable_matches_pin(path: &Path) -> Result<bool, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.to_string()),
    };
    if is_reparse(&metadata) || !metadata.is_file() {
        return Err(format!(
            "{} is not a regular non-link executable",
            path.display()
        ));
    }
    if metadata.len() != EPIC_EXECUTABLE_SIZE {
        return Ok(false);
    }
    let bytes = read_bounded(path, EPIC_EXECUTABLE_SIZE)?
        .ok_or("EpicGamesStarter executable disappeared during verification")?;
    Ok(sha256_hex(&bytes) == EPIC_EXECUTABLE_SHA256)
}

fn ensure_epic_starter(
    http: &dyn Http,
    game_dir: &Path,
    workspace_id: &str,
) -> Result<PathBuf, String> {
    let executable = game_dir.join("EpicGamesStarter.exe");
    if epic_executable_matches_pin(&executable)? {
        return Ok(executable);
    }
    let archive =
        download_resolved(http, &pinned_epic_download()?).map_err(|error| error.to_string())?;
    let delivered = validated_epic_executable(&archive)?;
    if delivered.len() as u64 != EPIC_EXECUTABLE_SIZE
        || sha256_hex(&delivered) != EPIC_EXECUTABLE_SHA256
    {
        return Err("downloaded EpicGamesStarter executable failed the extracted-file pin".into());
    }
    workspace_is_stopped(workspace_id)?;
    atomic_write(&executable, &delivered)?;
    if !epic_executable_matches_pin(&executable)? {
        return Err("published EpicGamesStarter failed pin verification".into());
    }
    Ok(executable)
}

/// Create a neutral token cache without replacing an existing Legendary login.
/// This keeps EpicGamesStarter away from Among Us's incompatible `EGSAuth.json`.
fn ensure_epic_auth_file(token_store: &Path) -> Result<PathBuf, String> {
    match fs::symlink_metadata(token_store) {
        Ok(metadata) if !is_reparse(&metadata) && metadata.is_file() => {
            return Ok(token_store.to_path_buf());
        }
        Ok(_) => {
            return Err(format!(
                "{} is not a regular non-link file",
                token_store.display()
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.to_string()),
    }

    atomic_write(token_store, b"null\n")?;
    Ok(token_store.to_path_buf())
}

fn ensure_epic_auth_store(user_profile: &Path) -> Result<PathBuf, String> {
    ensure_epic_auth_file(
        &user_profile
            .join(".config")
            .join("legendary")
            .join("user.json"),
    )
}

fn prepare_epic_auth_stores(
    game_dir: &Path,
    context: &compat::RuntimeContext,
) -> Result<(), String> {
    if context.host == compat::HostPlatform::Windows {
        let user_profile = std::env::var_os("USERPROFILE")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .ok_or("Windows USERPROFILE is unavailable; cannot prepare Epic authentication")?;
        ensure_epic_auth_store(&user_profile)?;
        return Ok(());
    }

    let mut prepared_profiles = 0_usize;
    if let Some(prefix) = &context.prefix {
        let users = prefix.join("drive_c").join("users");
        let entries = match fs::read_dir(&users) {
            Ok(entries) => Some(entries),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(format!(
                    "couldn't inspect Wine user profiles at {}: {error}",
                    users.display()
                ));
            }
        };
        if let Some(entries) = entries {
            for entry in entries {
                let entry = entry.map_err(|error| error.to_string())?;
                let metadata =
                    fs::symlink_metadata(entry.path()).map_err(|error| error.to_string())?;
                if is_reparse(&metadata) || !metadata.is_dir() {
                    continue;
                }
                let among_us_data = entry
                    .path()
                    .join("AppData")
                    .join("LocalLow")
                    .join("Innersloth")
                    .join("Among Us");
                let has_among_us_data = fs::symlink_metadata(&among_us_data)
                    .is_ok_and(|metadata| !is_reparse(&metadata) && metadata.is_dir());
                if has_among_us_data {
                    ensure_epic_auth_store(&entry.path())?;
                    prepared_profiles += 1;
                }
            }
        }
    }

    if prepared_profiles == 0 {
        ensure_epic_auth_file(&game_dir.join("EGSAuth.json"))?;
    }
    Ok(())
}

fn validate_managed_launch_target(game_dir: &Path, workspace_id: &str) -> Result<(), String> {
    let active = managed_instance::workspace_game_dir(workspace_id)?;
    if !same_path(game_dir, &active) {
        return Err("refusing to launch outside the managed profile workspace".into());
    }
    managed_instance::active_marker(workspace_id)?
        .ok_or("the managed profile workspace has no validated marker")?;
    validate_game_dir(game_dir)
}

fn launch_prepared_game(
    game_dir: &Path,
    instance: &settings::GameInstance,
    workspace_id: &str,
) -> Result<(), String> {
    workspace_is_stopped(workspace_id)?;
    validate_managed_launch_target(game_dir, workspace_id)?;
    let store = instance.store;
    let context = compat::resolve_with_hint(Path::new(&instance.path), Some(instance.runtime));
    let launcher = context
        .launcher
        .as_deref()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .unwrap_or("unresolved");
    log::info!(
        target: "perfect_sync::support",
        "launch target resolved; profile={workspace_id}; store={:?}; runtime={:?}; arch={:?}; host={:?}; launcher={launcher}; prefix={}",
        instance.store,
        context.runtime,
        instance.arch,
        context.host,
        context.prefix.is_some(),
    );
    if store == Store::Epic {
        let starter = ensure_epic_starter(&http()?, game_dir, workspace_id)?;
        prepare_epic_auth_stores(game_dir, &context)?;
        if cfg!(windows) {
            return spawn_launch(workspace_id, || {
                let helper_pid = process::launch_console_interactive(&starter, game_dir)
                    .map_err(|error| format!("couldn't run EpicGamesStarter: {error}"))?;
                crate::console_monitor::wait_for_game_and_submit_enter(
                    helper_pid,
                    game_dir,
                    Duration::from_secs(300),
                )
                .map_err(|error| format!("Epic launch failed: {error}"))
            });
        }
        let specification = compat::build_program_spec(&starter, game_dir, &context);
        return spawn_launch(workspace_id, || {
            if context.runtime == Runtime::Crossover {
                let launcher = launch_crossover(&specification, true)
                    .map_err(|error| launch_err_msg(&context, &error))?;
                supervise_crossover_launch(launcher, game_dir, CROSSOVER_GAME_START_TIMEOUT)
            } else {
                process::launch_interactive(&specification)
                    .map(|_| ())
                    .map_err(|error| launch_err_msg(&context, &error))
            }
        });
    }
    if store == Store::Msstore {
        if !game::is_writable_game_dir(game_dir) {
            return Err(
                "Microsoft Store/Game Pass installs must use a writable managed workspace.".into(),
            );
        }
        if game::exe_arch(&game_dir.join(process::GAME_EXE)) != Some(Arch::X64) {
            return Err(
                "The managed Microsoft Store workspace is not the expected x64 Among Us build."
                    .into(),
            );
        }
    }
    let specification = compat::build_launch_spec(game_dir, &context);
    if context.runtime == Runtime::Crossover {
        return spawn_launch(workspace_id, || {
            if store == Store::Steam && context.host == compat::HostPlatform::Macos {
                ensure_crossover_steam_ready(Path::new(&instance.path), &context)?;
            }
            let launcher = launch_crossover(&specification, false)
                .map_err(|error| launch_err_msg(&context, &error))?;
            supervise_crossover_launch(launcher, game_dir, CROSSOVER_GAME_START_TIMEOUT)
        });
    }
    spawn_launch(workspace_id, || {
        process::launch(&specification)
            .map(|_| ())
            .map_err(|error| launch_err_msg(&context, &error))
    })
}

#[tauri::command]
pub async fn launch_profile(
    game_path: String,
    profile_id: String,
    on_progress: Channel<OperationProgress>,
) -> Result<Option<String>, String> {
    blocking(move || {
        let _guard = lock_profile_mutation(&profile_id)?;
        let reporter = ProgressReporter::new(on_progress);
        if let Some(guidance) =
            prepare_profile(&game_path, &profile_id, Some(&reporter), false)?
        {
            return Ok(Some(guidance));
        }
        workspace_is_stopped(&profile_id)?;
        let profile = recovered_profile_store(&settings::profiles_root())?
            .load(&profile_id)
            .map_err(|error| error.to_string())?
            .ok_or("profile not found")?;
        let instance = saved_profile_instance(&game_path, &profile)?;
        let game_dir = managed_instance::workspace_game_dir(&profile_id)?;
        reporter.stage(
            "finalizing",
            if instance.store == Store::Epic {
                "Waiting for Epic authentication and Among Us startup. Complete sign-in in EpicGamesStarter if prompted."
            } else if cfg!(target_os = "macos")
                && instance.runtime == Runtime::Crossover
                && instance.store == Store::Steam
            {
                "Starting Steam and Among Us through CrossOver. Sign in to Steam if prompted."
            } else if instance.runtime == Runtime::Crossover {
                "Starting Among Us through CrossOver. Cold bottles can take up to five minutes."
            } else {
                "Starting Among Us"
            },
        );
        launch_prepared_game(&game_dir, &instance, &profile_id)?;
        Ok(None)
    })
    .await
}

#[tauri::command]
pub async fn launch_vanilla(
    game_path: String,
    profile_id: String,
    on_progress: Channel<OperationProgress>,
) -> Result<(), String> {
    blocking(move || {
        let _guard = lock_profile_mutation(&profile_id)?;
        let reporter = ProgressReporter::new(on_progress);
        validate_profile_id(&profile_id)?;
        reporter.stage(
            "preparing",
            "Creating or validating the private vanilla workspace",
        );
        let (game_dir, instance) = prepare_vanilla(&game_path, &profile_id)?;
        workspace_is_stopped(&profile_id)?;
        reporter.stage(
            "finalizing",
            if instance.store == Store::Epic {
                "Waiting for Epic authentication and vanilla Among Us startup. Complete sign-in in EpicGamesStarter if prompted."
            } else if cfg!(target_os = "macos")
                && instance.runtime == Runtime::Crossover
                && instance.store == Store::Steam
            {
                "Starting Steam and vanilla Among Us through CrossOver. Sign in to Steam if prompted."
            } else if instance.runtime == Runtime::Crossover {
                "Starting vanilla Among Us through CrossOver. Cold bottles can take up to five minutes."
            } else {
                "Starting vanilla Among Us"
            },
        );
        launch_prepared_game(&game_dir, &instance, &profile_id)
    })
    .await
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveBackupInfo {
    pub id: String,
    pub created_at: u64,
    pub files: usize,
    pub bytes: u64,
}

fn unix_millis() -> Result<u64, String> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_millis();
    u64::try_from(millis).map_err(|_| "system timestamp is too large".to_string())
}

fn innersloth_save_dir() -> Result<PathBuf, String> {
    let local = std::env::var_os("LOCALAPPDATA")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .ok_or("Windows LOCALAPPDATA is unavailable; save data cannot be located.")?;
    let local_low = local
        .parent()
        .ok_or("Windows LOCALAPPDATA has no parent directory.")?
        .join("LocalLow");
    Ok(local_low.join("Innersloth").join("Among Us"))
}

fn save_backups_root() -> PathBuf {
    settings::app_data_dir().join("backups").join("save-data")
}

fn create_save_backup_impl() -> Result<SaveBackupInfo, String> {
    let source = innersloth_save_dir()?;
    let metadata = fs::symlink_metadata(&source).map_err(|error| {
        format!(
            "Among Us save data was not found at {}: {error}",
            source.display()
        )
    })?;
    if is_reparse(&metadata) || !metadata.is_dir() {
        return Err("The Among Us save-data folder is not a safe regular directory.".into());
    }
    let created_at = unix_millis()?;
    let id = format!(
        "{created_at}-{}",
        TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    );
    let root = save_backups_root();
    fs::create_dir_all(&root).map_err(|error| error.to_string())?;
    let destination = root.join(&id);
    let stage = root.join(format!(".stage-{id}"));
    let result = (|| {
        fs::create_dir(&stage).map_err(|error| error.to_string())?;
        let mut files = 0;
        let mut bytes = 0;
        copy_game_tree(&source, &stage.join("data"), &mut files, &mut bytes)?;
        let info = SaveBackupInfo {
            id: id.clone(),
            created_at,
            files,
            bytes,
        };
        let manifest = serde_json::to_vec_pretty(&info).map_err(|error| error.to_string())?;
        atomic_write(&stage.join("manifest.json"), &manifest)?;
        fs::rename(&stage, &destination).map_err(|error| error.to_string())?;
        Ok(info)
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&stage);
    }
    result
}

#[tauri::command]
pub async fn backup_save_data() -> Result<SaveBackupInfo, String> {
    blocking(move || {
        let _guard = lock_mutations()?;
        all_games_are_stopped()?;
        let backup = create_save_backup_impl()?;
        if let Err(error) = prune_save_backups() {
            log::warn!("created save backup but could not prune old backups: {error}");
        }
        Ok(backup)
    })
    .await
}

fn prune_save_backups() -> Result<(), String> {
    let root = save_backups_root();
    let mut backups = list_save_backups_impl()?;
    if backups.len() <= 25 {
        return Ok(());
    }
    backups.sort_by_key(|backup| backup.created_at);
    let excess = backups.len().saturating_sub(25);
    for backup in backups.into_iter().take(excess) {
        let path = root.join(backup.id);
        let metadata = fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
        if is_reparse(&metadata) || !metadata.is_dir() {
            return Err("A save-backup path is not a safe regular directory.".into());
        }
        fs::remove_dir_all(path).map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn list_save_backups_impl() -> Result<Vec<SaveBackupInfo>, String> {
    let root = save_backups_root();
    let entries = match fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.to_string()),
    };
    let mut backups = Vec::new();
    for entry in entries.take(256) {
        let entry = entry.map_err(|error| error.to_string())?;
        let metadata = fs::symlink_metadata(entry.path()).map_err(|error| error.to_string())?;
        if is_reparse(&metadata) || !metadata.is_dir() {
            continue;
        }
        let Some(bytes) = read_bounded(&entry.path().join("manifest.json"), 64 * 1024)? else {
            continue;
        };
        let info: SaveBackupInfo =
            serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
        if info.id == entry.file_name().to_string_lossy() {
            backups.push(info);
        }
    }
    backups.sort_by(|left, right| right.created_at.cmp(&left.created_at));
    Ok(backups)
}

#[tauri::command]
pub async fn list_save_backups() -> Result<Vec<SaveBackupInfo>, String> {
    blocking(list_save_backups_impl).await
}

#[tauri::command]
pub async fn restore_save_data(backup_id: String) -> Result<SaveBackupInfo, String> {
    blocking(move || {
        let _guard = lock_mutations()?;
        all_games_are_stopped()?;
        if backup_id.is_empty()
            || backup_id.len() > 64
            || !backup_id
                .chars()
                .all(|character| character.is_ascii_digit() || character == '-')
        {
            return Err("Invalid save backup id.".into());
        }
        let backup = list_save_backups_impl()?
            .into_iter()
            .find(|backup| backup.id == backup_id)
            .ok_or("Save backup was not found.")?;
        let backup_data = save_backups_root().join(&backup.id).join("data");
        let backup_metadata =
            fs::symlink_metadata(&backup_data).map_err(|error| error.to_string())?;
        if is_reparse(&backup_metadata) || !backup_metadata.is_dir() {
            return Err("Save backup data is not a safe regular directory.".into());
        }

        let target = innersloth_save_dir()?;
        let parent = target
            .parent()
            .ok_or("Among Us save-data folder has no parent directory.")?;
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        let safety_backup = if target.is_dir() {
            Some(create_save_backup_impl()?)
        } else {
            None
        };
        let stage = unique_sibling(&target, "restore")?;
        let rollback = unique_sibling(&target, "rollback")?;
        let mut files = 0;
        let mut bytes = 0;
        if let Err(error) = copy_game_tree(&backup_data, &stage, &mut files, &mut bytes) {
            let _ = fs::remove_dir_all(&stage);
            return Err(error);
        }
        let had_target = target.exists();
        if had_target {
            let metadata = fs::symlink_metadata(&target).map_err(|error| error.to_string())?;
            if is_reparse(&metadata) || !metadata.is_dir() {
                let _ = fs::remove_dir_all(&stage);
                return Err("Current save data is not a safe regular directory.".into());
            }
            fs::rename(&target, &rollback).map_err(|error| error.to_string())?;
        }
        if let Err(error) = fs::rename(&stage, &target) {
            if had_target {
                let _ = fs::rename(&rollback, &target);
            }
            let _ = fs::remove_dir_all(&stage);
            return Err(format!("Could not publish restored save data: {error}"));
        }
        if had_target {
            if let Err(error) = fs::remove_dir_all(&rollback) {
                log::warn!("restored save data but could not remove rollback folder: {error}");
            }
        }
        if let Err(error) = prune_save_backups() {
            log::warn!("restored save data but could not prune old backups: {error}");
        }
        Ok(safety_backup.unwrap_or(backup))
    })
    .await
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticGame {
    pub name: String,
    pub store: Store,
    pub arch: Arch,
    pub runtime: Runtime,
    pub build: Option<String>,
    pub writable: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticLoader {
    pub current: bool,
    pub installed_version: Option<String>,
    pub winhttp: bool,
    pub preloader: bool,
    pub dotnet: bool,
    pub profile_plugins: usize,
    pub game_plugins: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticAsset {
    pub name: String,
    pub version: String,
    pub file: Option<String>,
    pub enabled: bool,
    pub source: ModSource,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsReport {
    pub generated_at: u64,
    pub app_version: String,
    pub profile_name: Option<String>,
    pub game: Option<DiagnosticGame>,
    pub loader: Option<DiagnosticLoader>,
    pub assets: Vec<DiagnosticAsset>,
    pub log_errors: Vec<String>,
    pub game_running: Option<bool>,
    pub warnings: Vec<String>,
}

fn count_dll_files(directory: &Path) -> Result<usize, String> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error.to_string()),
    };
    let mut count = 0;
    for entry in entries {
        let entry = entry.map_err(|error| error.to_string())?;
        let metadata = fs::symlink_metadata(entry.path()).map_err(|error| error.to_string())?;
        if !is_reparse(&metadata)
            && metadata.is_file()
            && entry
                .path()
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("dll"))
        {
            count += 1;
        }
    }
    Ok(count)
}

fn redact_sensitive(mut text: String, saved: &Settings) -> String {
    for path in saved
        .game_instances
        .iter()
        .map(|instance| instance.path.as_str())
        .chain(
            saved
                .personal_local_mods
                .iter()
                .map(|local| local.path.as_str()),
        )
    {
        if !path.is_empty() {
            text = text.replace(path, "<redacted-path>");
        }
    }
    redact_user_paths_and_tokens(text)
}

fn recent_log_errors(game_dir: &Path, saved: &Settings) -> Result<Vec<String>, String> {
    const MAX_LOG_TAIL: u64 = 2 * 1024 * 1024;
    let path = game_dir.join("BepInEx").join("LogOutput.log");
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.to_string()),
    };
    if is_reparse(&metadata) || !metadata.is_file() {
        return Err("BepInEx LogOutput.log is not a safe regular file.".into());
    }
    let mut file = File::open(&path).map_err(|error| error.to_string())?;
    if metadata.len() > MAX_LOG_TAIL {
        file.seek(SeekFrom::End(-(MAX_LOG_TAIL as i64)))
            .map_err(|error| error.to_string())?;
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    let text = String::from_utf8_lossy(&bytes);
    let mut errors: Vec<String> = text
        .lines()
        .filter(|line| {
            let folded = line.to_ascii_lowercase();
            folded.contains("[error") || folded.contains("exception")
        })
        .map(|line| redact_sensitive(line.trim().to_string(), saved))
        .collect();
    if errors.len() > 50 {
        errors.drain(..errors.len() - 50);
    }
    Ok(errors)
}

fn diagnostics_report_impl(profile_id: Option<&str>) -> Result<DiagnosticsReport, String> {
    if let Some(profile_id) = profile_id {
        validate_profile_id(profile_id)?;
    }
    let saved = settings::load().map_err(|error| error.to_string())?;
    let profile_store = recovered_profile_store(&settings::profiles_root())?;
    let profile = match profile_id {
        Some(profile_id) => profile_store
            .load(profile_id)
            .map_err(|error| error.to_string())?,
        None => None,
    };
    let instance = match profile.as_ref() {
        Some(profile) => profile.game_instance_id.as_deref().and_then(|id| {
            saved
                .game_instances
                .iter()
                .find(|instance| instance.id == id)
        }),
        None => saved.game_instances.first(),
    };
    let mut warnings = Vec::new();
    let mut loader_status = None;
    let mut log_errors = Vec::new();
    let game_status = if let Some(instance) = instance {
        match canonical_game_path(Path::new(&instance.path)) {
            Ok(game_dir) => {
                let build = game::detect_build(&game_dir);
                let writable = game::is_writable_game_dir(&game_dir);
                let workspace_dir = if let Some(profile) = profile.as_ref() {
                    managed_instance::active_marker(&profile.id)?
                        .filter(|marker| marker.profile_id == profile.id)
                        .map(|_| managed_instance::workspace_game_dir(&profile.id))
                        .transpose()?
                } else {
                    None
                };
                if let Some(profile) = profile.as_ref() {
                    let profile_dir = profile_store
                        .profile_dir(&profile.id)
                        .map_err(|error| error.to_string())?;
                    if let Some(workspace) = workspace_dir.as_ref() {
                        loader_status = Some(DiagnosticLoader {
                            current: loader::has_loader(workspace),
                            installed_version: loader::installed_version(workspace),
                            winhttp: workspace.join("winhttp.dll").is_file(),
                            preloader: workspace
                                .join("BepInEx")
                                .join("core")
                                .join(loader::IL2CPP_PRELOADER)
                                .is_file(),
                            dotnet: workspace.join("dotnet").join("coreclr.dll").is_file(),
                            profile_plugins: count_dll_files(
                                &profile_dir.join("BepInEx").join("plugins"),
                            )?,
                            game_plugins: count_dll_files(
                                &workspace.join("BepInEx").join("plugins"),
                            )?,
                        });
                        log_errors = recent_log_errors(workspace, &saved)?;
                    } else {
                        warnings.push(
                            "This profile's isolated workspace has not been prepared.".to_string(),
                        );
                    }
                }
                Some(DiagnosticGame {
                    name: instance.name.clone(),
                    store: instance.store,
                    arch: instance.arch,
                    runtime: instance.runtime,
                    build,
                    writable,
                })
            }
            Err(error) => {
                warnings.push(error);
                None
            }
        }
    } else {
        warnings.push("No Among Us instance is configured.".to_string());
        None
    };
    if let Some(loader) = loader_status.as_ref() {
        if !loader.current || !loader.winhttp || !loader.preloader || !loader.dotnet {
            warnings.push(
                "BepInEx is incomplete or not current in the isolated workspace.".to_string(),
            );
        }
    }
    let game_running = match process::try_is_running() {
        Ok(running) => Some(running),
        Err(error) => {
            warnings.push(format!("Could not check the Among Us process: {error}"));
            None
        }
    };
    let assets = profile
        .as_ref()
        .map(|profile| {
            profile
                .mods
                .iter()
                .map(|asset| DiagnosticAsset {
                    name: asset.name.clone(),
                    version: asset.version.clone(),
                    file: asset.file.clone(),
                    enabled: asset.enabled,
                    source: asset.source,
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(DiagnosticsReport {
        generated_at: unix_millis()?,
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        profile_name: profile.as_ref().map(|profile| profile.name.clone()),
        game: game_status,
        loader: loader_status,
        assets,
        log_errors,
        game_running,
        warnings,
    })
}

#[tauri::command]
pub async fn collect_diagnostics(profile_id: Option<String>) -> Result<DiagnosticsReport, String> {
    blocking(move || diagnostics_report_impl(profile_id.as_deref())).await
}

fn remove_regular_file_if_present(path: &Path) -> Result<(), String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
    };
    if is_reparse(&metadata) || !metadata.is_file() {
        return Err(format!("{} is not a regular file", path.display()));
    }
    fs::remove_file(path).map_err(|error| error.to_string())
}

fn refresh_bepinex_support_log(log_dir: &Path, profile_id: Option<&str>) -> Result<(), String> {
    let destination = log_dir.join("bepinex.log");
    let status_path = log_dir.join("bepinex-status.txt");
    let source = match profile_id {
        Some(profile_id) => {
            validate_profile_id(profile_id)?;
            managed_instance::workspace_game_dir(profile_id)?
                .join("BepInEx")
                .join("LogOutput.log")
        }
        None => {
            remove_regular_file_if_present(&destination)?;
            return atomic_write(
                &status_path,
                b"No profile was selected when this support snapshot was created.\n",
            );
        }
    };
    let metadata = match fs::symlink_metadata(&source) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            remove_regular_file_if_present(&destination)?;
            return atomic_write(
                &status_path,
                b"BepInEx LogOutput.log is not available yet. Launch the profile before collecting logs.\n",
            );
        }
        Err(error) => return Err(error.to_string()),
    };
    if is_reparse(&metadata) || !metadata.is_file() || metadata.len() == 0 {
        remove_regular_file_if_present(&destination)?;
        return atomic_write(
            &status_path,
            b"BepInEx LogOutput.log is not a usable regular file.\n",
        );
    }
    if metadata.len() > MAX_ERROR_LOG_BYTES {
        remove_regular_file_if_present(&destination)?;
        return atomic_write(
            &status_path,
            b"BepInEx LogOutput.log exceeds the 256 MiB support-log limit.\n",
        );
    }
    copy_error_log(&source, &destination, metadata.len())?;
    remove_regular_file_if_present(&status_path)
}

fn redacted_settings_value(saved: &Settings) -> Result<serde_json::Value, String> {
    let mut redacted = serde_json::to_value(saved).map_err(|error| error.to_string())?;
    if let Some(instances) = redacted
        .get_mut("gameInstances")
        .and_then(serde_json::Value::as_array_mut)
    {
        for instance in instances {
            if let Some(object) = instance.as_object_mut() {
                object.insert(
                    "path".to_string(),
                    serde_json::Value::String("<redacted-game-path>".to_string()),
                );
            }
        }
    }
    if let Some(locals) = redacted
        .get_mut("personalLocalMods")
        .and_then(serde_json::Value::as_array_mut)
    {
        for local in locals {
            if let Some(object) = local.as_object_mut() {
                object.insert(
                    "path".to_string(),
                    serde_json::Value::String("<redacted-local-mod-path>".to_string()),
                );
            }
        }
    }
    if redacted
        .get("storagePath")
        .is_some_and(|value| !value.is_null())
    {
        redacted["storagePath"] = serde_json::Value::String("<redacted-storage-path>".to_string());
    }
    Ok(redacted)
}

fn refresh_support_artifacts(log_dir: &Path, profile_id: Option<&str>) -> Result<(), String> {
    fs::create_dir_all(log_dir).map_err(|error| error.to_string())?;
    let metadata = fs::symlink_metadata(log_dir).map_err(|error| error.to_string())?;
    if is_reparse(&metadata) || !metadata.is_dir() {
        return Err("The support log location is not a regular directory.".into());
    }
    let diagnostics = match diagnostics_report_impl(profile_id) {
        Ok(report) => serde_json::to_vec_pretty(&report).map_err(|error| error.to_string())?,
        Err(error) => serde_json::to_vec_pretty(&serde_json::json!({
            "generatedAt": unix_millis()?,
            "appVersion": env!("CARGO_PKG_VERSION"),
            "error": redact_crossover_text(support_log_message(error)),
        }))
        .map_err(|error| error.to_string())?,
    };
    atomic_write(&log_dir.join("diagnostics.json"), &diagnostics)?;
    let saved = settings::load().map_err(|error| error.to_string())?;
    let redacted_settings = serde_json::to_vec_pretty(&redacted_settings_value(&saved)?)
        .map_err(|error| error.to_string())?;
    atomic_write(&log_dir.join("settings-redacted.json"), &redacted_settings)?;
    let profile_path = log_dir.join("profile.json");
    match profile_id {
        Some(profile_id) => {
            validate_profile_id(profile_id)?;
            match recovered_profile_store(&settings::profiles_root())?
                .load(profile_id)
                .map_err(|error| error.to_string())?
            {
                Some(profile) => atomic_write(
                    &profile_path,
                    &serde_json::to_vec_pretty(&profile).map_err(|error| error.to_string())?,
                )?,
                None => remove_regular_file_if_present(&profile_path)?,
            }
        }
        None => remove_regular_file_if_present(&profile_path)?,
    }
    refresh_bepinex_support_log(log_dir, profile_id)
}

fn open_directory(path: &Path) -> Result<(), String> {
    #[cfg(windows)]
    let mut command = process::command("explorer.exe");
    #[cfg(target_os = "macos")]
    let mut command = process::command("/usr/bin/open");
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = process::command("xdg-open");
    command.arg(path);
    let mut child = command
        .spawn()
        .map_err(|error| format!("Could not open the support logs folder: {error}"))?;
    std::thread::spawn(move || {
        if let Err(error) = child.wait() {
            log::warn!(
                target: "perfect_sync::support",
                "Could not reap the support-folder opener: {error}"
            );
        }
    });
    Ok(())
}

#[tauri::command]
pub async fn open_support_logs(
    app: AppHandle,
    profile_id: Option<String>,
) -> Result<String, String> {
    let log_dir = app
        .path()
        .app_log_dir()
        .map_err(|error| format!("Could not locate the support logs folder: {error}"))?;
    blocking(move || {
        refresh_support_artifacts(&log_dir, profile_id.as_deref())?;
        log::info!(
            target: "perfect_sync::support",
            "Support snapshot refreshed; profile={}",
            profile_id.as_deref().unwrap_or("none")
        );
        open_directory(&log_dir)?;
        Ok(log_dir.to_string_lossy().into_owned())
    })
    .await
}

#[tauri::command]
pub async fn export_support_bundle(
    destination: String,
    profile_id: Option<String>,
) -> Result<String, String> {
    blocking(move || {
        let path = PathBuf::from(&destination);
        if !path.is_absolute()
            || path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_none_or(|extension| !extension.eq_ignore_ascii_case("zip"))
        {
            return Err("Choose an absolute .zip destination for the support bundle.".into());
        }
        let report = diagnostics_report_impl(profile_id.as_deref())?;
        let saved = settings::load().map_err(|error| error.to_string())?;
        let redacted_settings = redacted_settings_value(&saved)?;
        let profile = match profile_id.as_deref() {
            Some(profile_id) => recovered_profile_store(&settings::profiles_root())?
                .load(profile_id)
                .map_err(|error| error.to_string())?,
            None => None,
        };
        let temporary = unique_sibling(&path, "support")?;
        let result = (|| {
            let output = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
                .map_err(|error| error.to_string())?;
            let mut archive = zip::ZipWriter::new(output);
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            archive
                .start_file("diagnostics.json", options)
                .map_err(|error| error.to_string())?;
            archive
                .write_all(&serde_json::to_vec_pretty(&report).map_err(|error| error.to_string())?)
                .map_err(|error| error.to_string())?;
            archive
                .start_file("settings-redacted.json", options)
                .map_err(|error| error.to_string())?;
            archive
                .write_all(
                    &serde_json::to_vec_pretty(&redacted_settings)
                        .map_err(|error| error.to_string())?,
                )
                .map_err(|error| error.to_string())?;
            if let Some(profile) = profile {
                archive
                    .start_file("profile.json", options)
                    .map_err(|error| error.to_string())?;
                archive
                    .write_all(
                        &serde_json::to_vec_pretty(&profile).map_err(|error| error.to_string())?,
                    )
                    .map_err(|error| error.to_string())?;
            }
            let output = archive.finish().map_err(|error| error.to_string())?;
            output.sync_all().map_err(|error| error.to_string())?;
            atomic_replace_file(&temporary, &path).map_err(|error| error.to_string())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result?;
        Ok(path.to_string_lossy().into_owned())
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crossover_launch_error_names_the_current_wine_launcher() {
        let context = compat::RuntimeContext {
            host: compat::HostPlatform::Macos,
            runtime: Runtime::Crossover,
            prefix: Some(PathBuf::from("/CrossOver/Bottles/AU")),
            launcher: None,
            launcher_args: vec!["--bottle".into(), "AU".into(), "--".into()],
        };
        let message = launch_err_msg(
            &context,
            &std::io::Error::from(std::io::ErrorKind::NotFound),
        );
        assert!(message.starts_with("Couldn't run CrossOver's Wine launcher"));
        assert!(message.contains("selected bottle and CrossOver installation"));
        assert!(!message.contains("cxrun"));
    }

    #[test]
    fn crossover_steam_preflight_uses_the_same_bottle_without_applaunch() {
        let temp = tempfile::tempdir().unwrap();
        let prefix = temp.path().join("CrossOver/Bottles/AU");
        let steam_root = prefix.join("drive_c/Program Files (x86)/Steam");
        let source_game_dir = steam_root.join("steamapps/common/Among Us");
        fs::create_dir_all(&source_game_dir).unwrap();
        let steam_client = steam_root.join("steam.exe");
        fs::write(&steam_client, b"steam").unwrap();
        let context = compat::RuntimeContext {
            host: compat::HostPlatform::Macos,
            runtime: Runtime::Crossover,
            prefix: Some(prefix),
            launcher: Some(PathBuf::from(
                "/Applications/CrossOver.app/Contents/SharedSupport/CrossOver/bin/wine",
            )),
            launcher_args: vec!["--bottle".into(), "AU".into(), "--".into()],
        };

        assert_eq!(
            crossover_steam_client(&source_game_dir, &context).unwrap(),
            steam_client
        );
        let specification = crossover_steam_launch_spec(&steam_client, &context).unwrap();
        assert_eq!(
            &specification.args[..6],
            [
                "--bottle",
                "AU",
                "--no-update",
                "--no-gui",
                "--wait-children",
                "--",
            ]
        );
        assert_eq!(Path::new(&specification.args[6]), steam_client);
        assert_eq!(specification.args[7], "-silent");
        assert!(!specification
            .args
            .iter()
            .any(|argument| argument == "-applaunch" || argument == "--dll"));
        assert!(specification
            .env
            .iter()
            .any(|(key, value)| key == "CX_BOTTLE" && value == "AU"));
    }

    #[test]
    fn successful_crossover_steam_wrapper_exit_remains_valid_during_handoff() {
        const CHILD: &str = "PERFECT_SYNC_CROSSOVER_STEAM_HANDOFF_CHILD";
        const TEST: &str =
            "commands::tests::successful_crossover_steam_wrapper_exit_remains_valid_during_handoff";

        if std::env::var_os(CHILD).is_some() {
            return;
        }

        let mut launcher = process::command(std::env::current_exe().unwrap())
            .env(CHILD, "1")
            .args(["--exact", TEST, "--nocapture"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut wrapper_exit = None;
        let deadline = Instant::now() + Duration::from_secs(2);
        while wrapper_exit.is_none() {
            poll_crossover_steam_wrapper(&mut launcher, &mut wrapper_exit).unwrap();
            assert!(
                Instant::now() < deadline,
                "successful wrapper exit was not observed"
            );
            std::thread::sleep(Duration::from_millis(10));
        }

        assert!(wrapper_exit.as_ref().unwrap().success());
        poll_crossover_steam_wrapper(&mut launcher, &mut wrapper_exit).unwrap();
    }

    #[test]
    fn support_log_messages_are_bounded_and_redacted() {
        let token = "github_pat_abcdefghijklmnopqrstuvwxyz123456";
        let home = ["USERPROFILE", "HOME", "APPDATA", "LOCALAPPDATA"]
            .into_iter()
            .find_map(std::env::var_os)
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default();
        let message = format!("{home}/Among Us\0 token={token}\n{}", "é".repeat(40_000));
        let sanitized = support_log_message(message);

        assert!(!sanitized.contains(token));
        assert!(!home.is_empty() && !sanitized.contains(&home));
        assert!(!sanitized.contains('\0'));
        assert!(sanitized.ends_with("[message truncated at 64 KiB]"));
        assert!(sanitized.len() <= MAX_SUPPORT_EVENT_BYTES + 40);
    }

    #[test]
    fn support_snapshot_explains_missing_profile_log() {
        let temp = tempfile::tempdir().unwrap();

        refresh_bepinex_support_log(temp.path(), None).unwrap();

        assert!(!temp.path().join("bepinex.log").exists());
        assert!(fs::read_to_string(temp.path().join("bepinex-status.txt"))
            .unwrap()
            .contains("No profile was selected"));
    }

    #[test]
    fn support_log_copy_is_a_stable_point_in_time_snapshot() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source.log");
        let destination = temp.path().join("copied.log");
        fs::write(&source, b"before-after").unwrap();

        copy_error_log(&source, &destination, 6).unwrap();

        assert_eq!(fs::read(destination).unwrap(), b"before");
    }

    #[test]
    fn support_settings_snapshot_redacts_storage_location() {
        let settings = Settings {
            storage_path: Some("C:/Users/private/Perfect Sync".into()),
            support_logging: true,
            ..Settings::default()
        };

        let redacted = redacted_settings_value(&settings).unwrap();

        assert_eq!(redacted["storagePath"], "<redacted-storage-path>");
        assert_eq!(redacted["supportLogging"], true);
    }

    #[test]
    fn ambiguous_storage_save_uses_only_the_reread_pointer() {
        let current = PathBuf::from("C:/old");
        let target = PathBuf::from("D:/new");

        assert_eq!(
            storage_root_after_ambiguous_save(
                Some("D:/new"),
                Some("C:/old"),
                Some("D:/new"),
                &current,
                &target,
            )
            .unwrap(),
            target
        );
        assert_eq!(
            storage_root_after_ambiguous_save(
                Some("C:/old"),
                Some("C:/old"),
                Some("D:/new"),
                &current,
                &target,
            )
            .unwrap(),
            current
        );
        assert!(storage_root_after_ambiguous_save(
            Some("E:/other"),
            Some("C:/old"),
            Some("D:/new"),
            &current,
            &target,
        )
        .is_err());
    }

    #[test]
    fn storage_move_lock_set_covers_profile_and_cache_shards() {
        let profile_guards = lock_all_profile_mutations().unwrap();
        assert_eq!(profile_guards.len(), PROFILE_LOCK_SHARDS);
        drop(profile_guards);
        let asset_guards = lock_all_asset_caches().unwrap();
        assert_eq!(asset_guards.len(), ASSET_CACHE_LOCK_SHARDS);
    }

    struct DownloadHttp(&'static [u8]);

    impl Http for DownloadHttp {
        fn get_text(
            &self,
            _url: &str,
        ) -> Result<String, perfect_sync_core::resolver::ResolveError> {
            unreachable!()
        }

        fn get_bytes(
            &self,
            _url: &str,
        ) -> Result<Vec<u8>, perfect_sync_core::resolver::ResolveError> {
            Ok(self.0.to_vec())
        }
    }

    struct MapHttp {
        metadata: String,
        bytes: &'static [u8],
    }

    impl Http for MapHttp {
        fn get_text(
            &self,
            _url: &str,
        ) -> Result<String, perfect_sync_core::resolver::ResolveError> {
            Ok(self.metadata.clone())
        }

        fn get_bytes(
            &self,
            _url: &str,
        ) -> Result<Vec<u8>, perfect_sync_core::resolver::ResolveError> {
            Ok(self.bytes.to_vec())
        }
    }

    #[test]
    fn profiles_resolve_only_their_explicit_game_instance() {
        let instance = |id: &str, name: &str, store: Store, arch: Arch| settings::GameInstance {
            id: id.into(),
            name: name.into(),
            path: format!("C:/{name}/Among Us"),
            executable_identity: None,
            source_fingerprint: None,
            source_file_count: None,
            source_byte_count: None,
            arch,
            store,
            runtime: Runtime::Native,
            build: Some("2026.3.31".into()),
            writable: true,
        };
        let saved = Settings {
            game_instances: vec![
                instance("steam", "Steam", Store::Steam, Arch::X86),
                instance("epic", "Epic", Store::Epic, Arch::X64),
            ],
            ..Settings::default()
        };

        let steam = selected_profile_instance(&saved, Some("steam")).unwrap();
        let epic = selected_profile_instance(&saved, Some("epic")).unwrap();
        assert_eq!((steam.store, steam.arch), (Store::Steam, Arch::X86));
        assert_eq!((epic.store, epic.arch), (Store::Epic, Arch::X64));
        assert_eq!(
            selected_profile_instance(&saved, None).unwrap_err(),
            "profile has no saved game instance"
        );
        assert!(selected_profile_instance(&saved, Some("missing")).is_err());
    }

    #[test]
    fn automatic_detection_excludes_modded_game_sources() {
        let temp = tempfile::tempdir().unwrap();
        let clean = temp.path().join("clean");
        let modded = temp.path().join("modded");
        fs::create_dir_all(&clean).unwrap();
        fs::create_dir_all(modded.join("BepInEx")).unwrap();
        let install = |path: PathBuf| game::GameInstall {
            path,
            store: Store::Steam,
            arch: Arch::X86,
            runtime: Runtime::Native,
            build: Some("test".into()),
            writable: true,
        };

        assert!(fresh_game_install_view(install(clean)).unwrap().is_some());
        assert!(fresh_game_install_view(install(modded)).unwrap().is_none());
    }

    #[test]
    fn error_log_copy_replaces_destination_with_exact_bytes() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("LogOutput.log");
        let destination = temp.path().join("export.log");
        fs::write(&source, b"BepInEx failure details\n").unwrap();
        fs::write(&destination, b"stale\n").unwrap();

        copy_error_log(&source, &destination, fs::metadata(&source).unwrap().len()).unwrap();

        assert_eq!(
            fs::read(&destination).unwrap(),
            b"BepInEx failure details\n"
        );
    }

    #[test]
    fn levelimposter_banner_proxy_accepts_only_official_bounded_images() {
        let official = "https://firebasestorage.googleapis.com/v0/b/levelimposter-347807.appspot.com/o/maps%2Fowner%2Fmap.png?alt=media&token=test";
        let data =
            levelimposter_banner_data_url(&DownloadHttp(b"\x89PNG\r\n\x1a\nimage"), official)
                .unwrap();
        assert!(data.starts_with("data:image/png;base64,"));
        assert!(levelimposter_banner_data_url(
            &DownloadHttp(b"\x89PNG\r\n\x1a\nimage"),
            "https://attacker.example/map.png",
        )
        .is_err());
        assert!(levelimposter_banner_data_url(&DownloadHttp(b"not an image"), official).is_err());
    }

    #[test]
    #[ignore]
    fn live_levelimposter_banner_proxy_fetches_current_map_image() {
        let http = UreqHttp::new(None);
        let text = http
            .get_text(&format!("{LEVELIMPOSTER_API}/maps/top"))
            .unwrap();
        let maps: Vec<LevelImposterMapMetadata> = levelimposter_callback(&text).unwrap();
        let banner = maps
            .into_iter()
            .find_map(|map| map.thumbnail_url)
            .expect("current map has a banner");

        let data = levelimposter_banner_data_url(&http, &banner).unwrap();

        assert!(data.starts_with("data:image/"));
        assert!(data.len() > 100);
    }

    #[test]
    fn levelimposter_map_download_requires_public_matching_storage_asset() {
        let id = "0ed1f569-eaf5-4ef6-b91c-f41ad78d4018";
        let metadata = |map_id: &str, url: &str, public: bool| {
            format!(
                r#"{{"v":1,"error":"","data":{{"id":"{map_id}","name":"Farm","isPublic":{public},"downloadURL":"{url}"}}}}"#
            )
        };
        let current = format!(
            "https://storage.googleapis.com/levelimposter-347807.appspot.com/maps/author/{id}.lim?signature=trusted"
        );
        let legacy = format!(
            "https://storage.googleapis.com/levelimposter/maps/{id}.lim2?signature=trusted"
        );
        for trusted in [&current, &legacy] {
            let http = MapHttp {
                metadata: metadata(id, trusted, true),
                bytes: b"map",
            };
            assert_eq!(
                levelimposter_map_download(&http, id).unwrap(),
                (id.to_string(), b"map".to_vec())
            );
        }

        let untrusted_bucket = format!("https://storage.googleapis.com/attacker/maps/{id}.lim");
        for (map_id, url, public) in [
            (
                "ef1d13ec-64ce-4c2c-a45d-816fc4ff46da",
                current.as_str(),
                true,
            ),
            (id, "https://example.invalid/map.lim2", true),
            (id, untrusted_bucket.as_str(), true),
            (id, current.as_str(), false),
        ] {
            let http = MapHttp {
                metadata: metadata(map_id, url, public),
                bytes: b"map",
            };
            assert!(levelimposter_map_download(&http, id).is_err());
        }
    }

    #[test]
    #[ignore]
    fn live_levelimposter_map_download_accepts_current_lim_asset() {
        let id = "c4ab53b1-1cc2-4080-9648-5f3d4ceab3d5";
        let (downloaded_id, bytes) = levelimposter_map_download(&UreqHttp::new(None), id).unwrap();

        assert_eq!(downloaded_id, id);
        assert!(!bytes.is_empty());
    }

    #[test]
    fn replacing_profile_maps_removes_only_previously_owned_maps() {
        let temp = tempfile::tempdir().unwrap();
        let profile_id = "maps";
        let directory = temp
            .path()
            .join(profile_id)
            .join("BepInEx/plugins/LevelImposter");
        fs::create_dir_all(&directory).unwrap();
        let old = "ef1d13ec-64ce-4c2c-a45d-816fc4ff46da";
        let unmanaged = "33eaaab4-b5fb-90f7-fb39-a41291409f93";
        let selected = "0ed1f569-eaf5-4ef6-b91c-f41ad78d4018";
        fs::write(directory.join(format!("{old}.lim2")), b"old").unwrap();
        fs::write(directory.join(format!("{unmanaged}.lim2")), b"user").unwrap();

        replace_profile_levelimposter_maps(
            temp.path(),
            profile_id,
            &[old.into()],
            &[(selected.into(), b"selected".to_vec())],
        )
        .unwrap();

        assert!(!directory.join(format!("{old}.lim2")).exists());
        assert_eq!(
            fs::read(directory.join(format!("{unmanaged}.lim2"))).unwrap(),
            b"user"
        );
        assert_eq!(
            fs::read(directory.join(format!("{selected}.lim2"))).unwrap(),
            b"selected"
        );
    }

    fn resolved_download(size: u64, digest: Option<&str>) -> ResolvedDownload {
        let digest = digest
            .map(|value| format!(r#","digest":"{value}""#))
            .unwrap_or_default();
        let release = resolver::parse_release(&format!(
            r#"{{"tag_name":"v1","assets":[{{"name":"mod.dll","browser_download_url":"https://example.invalid/mod.dll","size":{size}{digest}}}]}}"#
        ))
        .unwrap();
        let asset = &release.assets[0];
        ResolvedDownload {
            url: asset.url.clone(),
            asset_name: asset.name.clone(),
            version: release.tag.clone(),
            size: asset.size,
        }
    }

    #[test]
    fn resolved_install_verifies_metadata_before_publishing() {
        let temp = tempfile::tempdir().unwrap();
        let profile = "checked-download";

        let size_error = install_resolved(
            temp.path(),
            profile,
            &DownloadHttp(b"short"),
            &resolved_download(6, None),
            None,
        )
        .unwrap_err();
        assert!(size_error.contains("download size mismatch"));

        let digest_error = install_resolved(
            temp.path(),
            profile,
            &DownloadHttp(b"evil"),
            &resolved_download(
                4,
                Some("sha256:770e607624d689265ca6c44884d0807d9b054d23c473c106c72be9de08b7376c"),
            ),
            None,
        )
        .unwrap_err();
        assert!(digest_error.contains("SHA-256 digest"));
        assert!(!temp
            .path()
            .join(profile)
            .join("BepInEx")
            .join("plugins")
            .join("mod.dll")
            .exists());

        let mut zip = resolved_download(4, None);
        zip.asset_name = "mod.zip".into();
        assert_eq!(
            install_resolved(temp.path(), profile, &DownloadHttp(b"data"), &zip, None),
            Err("Catalog ZIP installs require an exact declared DLL name. Pick a DLL file manually.".into())
        );
    }

    #[test]
    fn local_mod_import_copies_only_a_bare_dll_and_records_file_source() {
        let temp = tempfile::tempdir().unwrap();
        let profiles_root = temp.path().join("profiles");
        let profile_id = "local-mod";
        ProfileStore::new(&profiles_root)
            .save(&ProfileRecord {
                id: profile_id.into(),
                name: "Local".into(),
                crew_color: "#fff".into(),
                game_build: None,
                game_instance_id: None,
                mods: Vec::new(),
                levelimposter_maps: Vec::new(),
            })
            .unwrap();
        let source = temp.path().join("CustomRoles.dll");
        fs::write(&source, b"local-dll").unwrap();

        let installed = install_local_mod_impl(&profiles_root, profile_id, &source).unwrap();

        assert_eq!(installed.mods.len(), 1);
        assert_eq!(installed.mods[0].package_id, "local/customroles.dll");
        assert_eq!(installed.mods[0].source, ModSource::File);
        assert_eq!(installed.mods[0].file.as_deref(), Some("CustomRoles.dll"));
        assert_eq!(
            fs::read(
                profiles_root
                    .join(profile_id)
                    .join("BepInEx/plugins/CustomRoles.dll")
            )
            .unwrap(),
            b"local-dll"
        );
        assert!(profile::to_manifest(&installed).mods.is_empty());
        let mut disabled = installed.clone();
        disabled.mods[0].enabled = false;
        assert!(profile::to_manifest(&disabled).mods.is_empty());

        let archive = temp.path().join("mod.zip");
        fs::write(&archive, b"not-a-dll").unwrap();
        assert!(install_local_mod_impl(&profiles_root, profile_id, &archive)
            .unwrap_err()
            .contains("DLL"));
    }

    #[test]
    fn lobby_digest_is_collision_resistant_and_stable() {
        assert_eq!(lobby_digest("abc"), lobby_digest("abc"));
        assert_ne!(lobby_digest("abc"), lobby_digest("abd"));
        assert_eq!(lobby_digest("anything").len(), 64);
    }

    #[test]
    fn town_of_us_dependencies_are_owned_by_the_complete_zip() {
        let catalog = bundled_catalog();
        let root = "AU-Avengers/TOU-Mira".to_string();
        let ordered = deps::resolve(&catalog, std::slice::from_ref(&root))
            .unwrap()
            .ordered;
        assert_eq!(ordered, [root]);
        assert!(tou_bundle_dependency("NuclearPowered/Reactor"));
        assert!(tou_bundle_dependency("All-Of-Us-Mods/MiraAPI"));
        assert!(tou_bundle_dependency("miniduikboot/Mini.RegionInstall"));
    }

    #[test]
    fn bundled_catalog_allows_user_selected_role_mod_combinations() {
        let catalog = bundled_catalog();
        validate_authoritative_dependencies(
            &catalog,
            &[
                "AU-Avengers/TOU-Mira".into(),
                "TheOtherRolesAU/TheOtherRoles".into(),
            ],
        )
        .unwrap();
    }

    #[test]
    fn bundled_catalog_exposes_the_expanded_verified_mod_set() {
        let catalog = bundled_catalog();
        assert_eq!(catalog.mods.len(), 33);
        for id in [
            "D1GQ/BetterAmongUs",
            "xChipseq/VanillaEnhancements",
            "Mr-Fluuff/StellarRolesAU",
            "SuperNewRoles/SuperNewRoles",
            "CallOfCreator/NewMod",
            "RaresHonour/HostGuard",
            "AtomicTyler1/MinimumLevel",
        ] {
            let entry = catalog.get(id).unwrap_or_else(|| panic!("missing {id}"));
            assert!(entry.repo.is_some());
            assert!(entry.asset_rules.dll_name.is_some());
        }
        for (id, name, trust) in [
            ("raspberrygitq/AleLuduMod", "AleLuduMod", Trust::Community),
            ("astra1dev/AUnlocker", "AUnlocker", Trust::Trusted),
            ("TwistAU/Submerged", "Mira Submerged", Trust::Community),
        ] {
            let entry = catalog.get(id).unwrap_or_else(|| panic!("missing {id}"));
            assert_eq!(entry.name, name);
            assert_eq!(entry.trust, trust);
        }
    }

    #[test]
    fn lobby_rows_only_mark_catalog_dependencies_as_managed() {
        let catalog = bundled_catalog();
        let selected = vec![
            "NuclearPowered/Reactor".to_string(),
            "All-Of-Us-Mods/MiraAPI".to_string(),
            "AU-Avengers/TOU-Mira".to_string(),
        ];
        let managed = selected_dependencies(&catalog, &selected).unwrap();
        assert!(managed.contains("nuclearpowered/reactor"));
        assert!(!managed.contains("all-of-us-mods/miraapi"));
        assert!(!managed.contains("au-avengers/tou-mira"));
    }

    #[test]
    fn dependency_normalization_prunes_orphans_but_keeps_explicit_roots() {
        let temp = tempfile::tempdir().unwrap();
        let profile_id = "deps";
        let plugin = |id: &str, file: &str, managed: bool| InstalledMod {
            package_id: id.into(),
            name: id.into(),
            repo: Some(id.into()),
            version: "v1".into(),
            versions: vec!["v1".into()],
            enabled: true,
            source: ModSource::Github,
            tags: Vec::new(),
            managed,
            update: None,
            file: Some(file.into()),
            asset: Some(file.into()),
        };
        for file in ["Tou.dll", "Reactor.dll", "Mira.dll"] {
            profile::install_plugin_bytes(temp.path(), profile_id, file, file.as_bytes()).unwrap();
        }
        let mut record = ProfileRecord {
            id: profile_id.into(),
            name: "Dependencies".into(),
            crew_color: "#fff".into(),
            game_build: None,
            game_instance_id: None,
            mods: vec![
                plugin("AU-Avengers/TOU-Mira", "Tou.dll", false),
                plugin("NuclearPowered/Reactor", "Reactor.dll", false),
                plugin("All-Of-Us-Mods/MiraAPI", "Mira.dll", true),
            ],
            levelimposter_maps: Vec::new(),
        };
        let catalog = bundled_catalog();
        normalize_dependency_ownership(temp.path(), profile_id, &mut record, &catalog).unwrap();
        assert!(record
            .mods
            .iter()
            .find(|item| item.package_id == "NuclearPowered/Reactor")
            .is_some_and(|item| !item.managed));
        record
            .mods
            .retain(|item| item.package_id != "AU-Avengers/TOU-Mira");
        profile::remove_plugin(temp.path(), profile_id, "Tou.dll").unwrap();
        normalize_dependency_ownership(temp.path(), profile_id, &mut record, &catalog).unwrap();
        assert_eq!(record.mods.len(), 1);
        assert_eq!(record.mods[0].package_id, "NuclearPowered/Reactor");
        assert!(!temp
            .path()
            .join(profile_id)
            .join("BepInEx/plugins/Mira.dll")
            .exists());
    }

    #[test]
    fn town_of_us_takeover_keeps_other_mod_dependencies_for_restoration() {
        let temp = tempfile::tempdir().unwrap();
        let profile_id = "tou-takeover";
        let catalog = parse(
            r#"{"schema":1,"mods":[
                {"id":"AU-Avengers/TOU-Mira","name":"Town of Us","summary":"","repo":"AU-Avengers/TOU-Mira","tags":[],"trust":"trusted","dependencies":[],"assetRules":{}},
                {"id":"Owner/OtherMod","name":"Other Mod","summary":"","repo":"Owner/OtherMod","tags":[],"trust":"trusted","dependencies":["All-Of-Us-Mods/MiraAPI","miniduikboot/Mini.RegionInstall"],"assetRules":{}},
                {"id":"All-Of-Us-Mods/MiraAPI","name":"MiraAPI","summary":"","repo":"All-Of-Us-Mods/MiraAPI","tags":[],"trust":"trusted","dependencies":["NuclearPowered/Reactor"],"assetRules":{}},
                {"id":"NuclearPowered/Reactor","name":"Reactor","summary":"","repo":"NuclearPowered/Reactor","tags":[],"trust":"trusted","dependencies":[],"assetRules":{}},
                {"id":"miniduikboot/Mini.RegionInstall","name":"Mini","summary":"","repo":"miniduikboot/Mini.RegionInstall","tags":[],"trust":"trusted","dependencies":[],"assetRules":{}}
            ]}"#,
        )
        .unwrap();
        let installed = |id: &str, file: &str, managed: bool| InstalledMod {
            package_id: id.into(),
            name: id.into(),
            repo: Some(id.into()),
            version: "v1".into(),
            versions: vec!["v1".into()],
            enabled: true,
            source: ModSource::Github,
            tags: Vec::new(),
            managed,
            update: None,
            file: Some(file.into()),
            asset: Some(file.into()),
        };
        let mut record = ProfileRecord {
            id: profile_id.into(),
            name: "Takeover".into(),
            crew_color: "#fff".into(),
            game_build: None,
            game_instance_id: None,
            mods: vec![
                installed("Owner/OtherMod", "Other.dll", false),
                installed("All-Of-Us-Mods/MiraAPI", "MiraAPI.dll", true),
                installed("NuclearPowered/Reactor", "Reactor.dll", true),
                installed(
                    "miniduikboot/Mini.RegionInstall",
                    "Mini.RegionInstall.dll",
                    true,
                ),
                installed("AU-Avengers/TOU-Mira", "TownOfUsMira.dll", false),
            ],
            levelimposter_maps: Vec::new(),
        };
        for file in [
            "Other.dll",
            "MiraAPI.dll",
            "Reactor.dll",
            "Mini.RegionInstall.dll",
            "TownOfUsMira.dll",
        ] {
            profile::install_plugin_bytes(temp.path(), profile_id, file, file.as_bytes()).unwrap();
        }

        normalize_dependency_ownership(temp.path(), profile_id, &mut record, &catalog).unwrap();
        assert_eq!(record.mods.len(), 5);
        record
            .mods
            .retain(|installed| !is_tou_mira(&installed.package_id));
        profile::remove_plugin(temp.path(), profile_id, "TownOfUsMira.dll").unwrap();
        normalize_dependency_ownership(temp.path(), profile_id, &mut record, &catalog).unwrap();

        assert_eq!(record.mods.len(), 4);
        for file in ["MiraAPI.dll", "Reactor.dll", "Mini.RegionInstall.dll"] {
            assert!(temp
                .path()
                .join(profile_id)
                .join("BepInEx/plugins")
                .join(file)
                .is_file());
        }
    }

    #[test]
    fn profile_transaction_failure_preserves_record_and_files() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("profiles");
        let store = ProfileStore::new(&root);
        let record = ProfileRecord {
            id: "stable".into(),
            name: "Old".into(),
            crew_color: "#fff".into(),
            game_build: None,
            game_instance_id: None,
            mods: Vec::new(),
            levelimposter_maps: Vec::new(),
        };
        store.save(&record).unwrap();
        profile::install_plugin_bytes(&root, "stable", "Owned.dll", b"old").unwrap();
        let original_manifest = fs::read(root.join("stable/profile.json")).unwrap();
        let original_plugin = fs::read(root.join("stable/BepInEx/plugins/Owned.dll")).unwrap();

        let error = profile_transaction(&root, "stable", |stage_root, _| {
            profile::remove_plugin(stage_root, "stable", "Owned.dll").unwrap();
            profile::install_plugin_bytes(stage_root, "stable", "Owned.dll", b"new").unwrap();
            Err::<(), _>("injected failure".into())
        })
        .unwrap_err();

        assert_eq!(error, "injected failure");
        assert_eq!(
            fs::read(root.join("stable/profile.json")).unwrap(),
            original_manifest
        );
        assert_eq!(
            fs::read(root.join("stable/BepInEx/plugins/Owned.dll")).unwrap(),
            original_plugin
        );
    }

    #[test]
    fn interrupted_mod_toggle_recovers_to_the_manifest_state() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("profiles");
        let id = "toggle-recovery";
        let file = "Toggle.dll";
        let mut record = ProfileRecord {
            id: id.into(),
            name: "Toggle".into(),
            crew_color: "#fff".into(),
            game_build: None,
            game_instance_id: None,
            mods: vec![InstalledMod {
                package_id: "Owner/Toggle".into(),
                name: "Toggle".into(),
                repo: Some("Owner/Toggle".into()),
                version: "v1".into(),
                versions: vec!["v1".into()],
                enabled: true,
                source: ModSource::Github,
                tags: Vec::new(),
                managed: false,
                update: None,
                file: Some(file.into()),
                asset: Some(file.into()),
            }],
            levelimposter_maps: Vec::new(),
        };
        let store = ProfileStore::new(&root);
        store.save(&record).unwrap();
        profile::install_plugin_bytes(&root, id, file, b"plugin").unwrap();

        let journal = write_mod_toggle_recovery_journal(&root, id, file).unwrap();
        let visible = recovered_profile_store(&root).unwrap().list().unwrap();
        assert!(visible[0].mods[0].enabled);
        assert!(root.join(id).join("BepInEx/plugins/Toggle.dll").is_file());
        assert!(!journal.exists());

        write_mod_toggle_recovery_journal(&root, id, file).unwrap();
        profile::set_plugin_enabled(&root, id, file, false).unwrap();
        let visible = recovered_profile_store(&root)
            .unwrap()
            .load(id)
            .unwrap()
            .unwrap();
        assert!(visible.mods[0].enabled);
        assert!(root.join(id).join("BepInEx/plugins/Toggle.dll").is_file());
        assert!(!journal.exists());

        write_mod_toggle_recovery_journal(&root, id, file).unwrap();
        profile::set_plugin_enabled(&root, id, file, false).unwrap();
        record.mods[0].enabled = false;
        store.save(&record).unwrap();
        let visible = recovered_profile_store(&root).unwrap().list().unwrap();
        assert!(!visible[0].mods[0].enabled);
        assert!(root
            .join(id)
            .join("BepInEx/plugins/Toggle.dll.disabled")
            .is_file());
        assert!(!journal.exists());
    }

    #[test]
    fn profile_transaction_artifacts_never_enter_the_profile_store() {
        use std::cell::RefCell;

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("profiles");
        let observed_stage = RefCell::new(None);
        profile_transaction(&root, "new-profile", |stage_root, stage_store| {
            observed_stage.replace(Some(stage_root.to_path_buf()));
            stage_store
                .save(&ProfileRecord {
                    id: "new-profile".into(),
                    name: "New".into(),
                    crew_color: "#fff".into(),
                    game_build: None,
                    game_instance_id: None,
                    mods: Vec::new(),
                    levelimposter_maps: Vec::new(),
                })
                .map_err(|error| error.to_string())
        })
        .unwrap();

        let stage = observed_stage.into_inner().unwrap();
        assert_eq!(stage.parent(), root.parent());
        assert_ne!(stage.parent(), Some(root.as_path()));
        let backup_candidate = unique_sibling(&root, "backup").unwrap();
        assert_eq!(backup_candidate.parent(), root.parent());
        assert_ne!(backup_candidate.parent(), Some(root.as_path()));
        assert!(!stage.exists());
        assert_eq!(ProfileStore::new(&root).list().unwrap().len(), 1);
        assert_eq!(
            fs::read_dir(&root)
                .unwrap()
                .map(|entry| entry.unwrap().file_name())
                .collect::<Vec<_>>(),
            vec![std::ffi::OsString::from("new-profile")]
        );
    }

    #[test]
    fn interrupted_profile_swap_recovers_before_list_and_load() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("profiles");
        let id = "recoverable";
        let original = ProfileRecord {
            id: id.into(),
            name: "Original".into(),
            crew_color: "#fff".into(),
            game_build: None,
            game_instance_id: None,
            mods: Vec::new(),
            levelimposter_maps: Vec::new(),
        };
        ProfileStore::new(&root).save(&original).unwrap();
        let paths = allocate_profile_transaction_paths(&root).unwrap();
        fs::create_dir(&paths.stage_root).unwrap();
        fs::create_dir(&paths.backup_root).unwrap();
        copy_profile_tree(&root.join(id), &paths.stage_root.join(id)).unwrap();
        let stage_store = ProfileStore::new(&paths.stage_root);
        let mut committed = stage_store.load(id).unwrap().unwrap();
        committed.name = "Committed".into();
        stage_store.save(&committed).unwrap();
        write_profile_recovery_journal(&paths.journal, id, ProfileRecoveryAction::Publish).unwrap();
        fs::rename(root.join(id), paths.backup_root.join(id)).unwrap();

        let recovered = recovered_profile_store(&root).unwrap();
        let listed = recovered.list().unwrap();

        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "Committed");
        assert_eq!(recovered.load(id).unwrap().unwrap().name, "Committed");
        assert!(!paths.stage_root.exists());
        assert!(!paths.backup_root.exists());
        assert!(!paths.journal.exists());
    }

    #[test]
    fn interrupted_profile_swap_restores_backup_when_publish_stage_is_gone() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("profiles");
        let id = "restore-original";
        let original = ProfileRecord {
            id: id.into(),
            name: "Original".into(),
            crew_color: "#fff".into(),
            game_build: None,
            game_instance_id: None,
            mods: Vec::new(),
            levelimposter_maps: Vec::new(),
        };
        ProfileStore::new(&root).save(&original).unwrap();
        let paths = allocate_profile_transaction_paths(&root).unwrap();
        fs::create_dir(&paths.stage_root).unwrap();
        fs::create_dir(&paths.backup_root).unwrap();
        write_profile_recovery_journal(&paths.journal, id, ProfileRecoveryAction::Publish).unwrap();
        fs::rename(root.join(id), paths.backup_root.join(id)).unwrap();

        let recovered = recovered_profile_store(&root).unwrap();

        assert_eq!(recovered.load(id).unwrap().unwrap().name, "Original");
        assert!(!paths.stage_root.exists());
        assert!(!paths.backup_root.exists());
        assert!(!paths.journal.exists());
    }

    #[test]
    fn ambiguous_profile_recovery_fails_closed_and_retains_every_artifact() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("profiles");
        let id = "ambiguous";
        let record = ProfileRecord {
            id: id.into(),
            name: "Original".into(),
            crew_color: "#fff".into(),
            game_build: None,
            game_instance_id: None,
            mods: Vec::new(),
            levelimposter_maps: Vec::new(),
        };
        ProfileStore::new(&root).save(&record).unwrap();
        let paths = allocate_profile_transaction_paths(&root).unwrap();
        fs::create_dir(&paths.stage_root).unwrap();
        fs::create_dir(&paths.backup_root).unwrap();
        copy_profile_tree(&root.join(id), &paths.stage_root.join(id)).unwrap();
        write_profile_recovery_journal(&paths.journal, id, ProfileRecoveryAction::Publish).unwrap();
        fs::rename(root.join(id), paths.backup_root.join(id)).unwrap();
        copy_profile_tree(&paths.backup_root.join(id), &root.join(id)).unwrap();

        let error = recovered_profile_store(&root).err().unwrap();

        assert!(error.contains("recovery evidence was retained"));
        assert!(root.join(id).exists());
        assert!(paths.stage_root.join(id).exists());
        assert!(paths.backup_root.join(id).exists());
        assert!(paths.journal.exists());
    }

    #[test]
    fn failed_commit_retains_backup_when_rollback_fails() {
        let temp = tempfile::tempdir().unwrap();
        let stage_root = temp.path().join("stage");
        let backup_root = temp.path().join("backup");
        let backup = backup_root.join("stable");
        fs::create_dir_all(&stage_root).unwrap();
        fs::create_dir_all(&backup).unwrap();
        fs::write(backup.join("profile.json"), b"intact").unwrap();

        let error = failed_profile_commit(
            &backup,
            &io::Error::other("publish blocked"),
            Err("rollback blocked".into()),
        );

        assert!(stage_root.exists());
        assert_eq!(fs::read(backup.join("profile.json")).unwrap(), b"intact");
        assert!(error.contains(&backup.display().to_string()));
    }

    #[test]
    fn failed_game_prepare_restores_all_touched_artifacts() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("BepInEx/config")).unwrap();
        fs::write(temp.path().join("winhttp.dll"), b"old-loader").unwrap();
        fs::write(
            temp.path().join("BepInEx/config/BepInEx.cfg"),
            b"old-config",
        )
        .unwrap();
        let error = game_artifact_transaction(temp.path(), || {
            fs::write(temp.path().join("winhttp.dll"), b"new-loader").unwrap();
            fs::create_dir_all(temp.path().join("BepInEx/plugins")).unwrap();
            fs::write(temp.path().join("BepInEx/plugins/New.dll"), b"new").unwrap();
            Err::<(), _>("injected prepare failure".into())
        })
        .unwrap_err();
        assert_eq!(error, "injected prepare failure");
        assert_eq!(
            fs::read(temp.path().join("winhttp.dll")).unwrap(),
            b"old-loader"
        );
        assert_eq!(
            fs::read(temp.path().join("BepInEx/config/BepInEx.cfg")).unwrap(),
            b"old-config"
        );
        assert!(!temp.path().join("BepInEx/plugins/New.dll").exists());
    }

    fn epic_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, bytes) in entries {
            writer
                .start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    #[test]
    fn epic_helper_zip_requires_one_exact_nonempty_executable() {
        assert_eq!(
            validated_epic_executable(&epic_zip(&[("EpicGamesStarter.exe", b"verified")])).unwrap(),
            b"verified"
        );
        for invalid in [
            epic_zip(&[("other.exe", b"x")]),
            epic_zip(&[("EpicGamesStarter.exe", b"")]),
        ] {
            assert!(validated_epic_executable(&invalid).is_err());
        }
        let pin = pinned_epic_download().unwrap();
        assert_eq!(pin.url, EPIC_STARTER_URL);
        assert_eq!(pin.size.bytes(), EPIC_STARTER_SIZE);
    }

    #[test]
    fn bundled_install_policy_overrides_stale_town_of_us_policy() {
        let mut stale = parse(
            r#"{"schema":1,"mods":[{"id":"NuclearPowered/Reactor","name":"Reactor","summary":"","repo":"NuclearPowered/Reactor","tags":[],"trust":"trusted","dependencies":[],"assetRules":{}},{"id":"AU-Avengers/TOU-Mira","name":"Town of Us - Mira","summary":"stale","repo":"AU-Avengers/TOU-Mira","tags":[],"trust":"trusted","dependencies":["NuclearPowered/Reactor"],"assetRules":{"perArch":{},"dllName":"Wrong.dll","bundlesLoader":false}}]}"#,
        )
        .unwrap();

        apply_bundled_install_policy(&mut stale);

        let entry = stale.get("AU-Avengers/TOU-Mira").unwrap();
        assert_eq!(entry.asset_rules.per_arch.len(), 2);
        assert_eq!(
            entry.asset_rules.dll_name.as_deref(),
            Some("TownOfUsMira.dll")
        );
        assert!(entry.asset_rules.bundles_loader);
        assert!(entry.dependencies.is_empty());
    }

    #[test]
    fn bundled_display_policy_exposes_current_town_of_us_bundle_contents() {
        let mut item = catalog_item(
            bundled_catalog()
                .get("AU-Avengers/TOU-Mira")
                .unwrap()
                .clone(),
        );
        item.dependencies.push("stale/dependency".into());
        item.included.clear();

        apply_bundled_display_policy(std::slice::from_mut(&mut item));

        assert!(item.dependencies.is_empty());
        assert_eq!(
            item.included,
            [
                "MiraAPI",
                "Reactor",
                "Mini.RegionInstall with the Town of Us server config",
                "Town of Us cosmetics"
            ]
        );
    }

    #[test]
    fn update_discovery_skips_asset_download_probes() {
        struct ReleaseHttp;

        impl Http for ReleaseHttp {
            fn get_text(
                &self,
                _url: &str,
            ) -> Result<String, perfect_sync_core::resolver::ResolveError> {
                unreachable!()
            }

            fn get_text_fresh(
                &self,
                _url: &str,
            ) -> Result<String, perfect_sync_core::resolver::ResolveError> {
                Ok(r#"<a href="/AU-Avengers/TOU-Mira/releases/download/1.7.0/TouMira.v1.7.0-x86-steam-itch.zip">package</a>"#.into())
            }

            fn get_text_with_url_fresh(
                &self,
                _url: &str,
            ) -> Result<
                perfect_sync_core::resolver::TextResponse,
                perfect_sync_core::resolver::ResolveError,
            > {
                Ok(perfect_sync_core::resolver::TextResponse {
                    body: String::new(),
                    final_url: "https://github.com/AU-Avengers/TOU-Mira/releases/tag/1.7.0".into(),
                })
            }

            fn get_bytes(
                &self,
                _url: &str,
            ) -> Result<Vec<u8>, perfect_sync_core::resolver::ResolveError> {
                unreachable!()
            }
        }

        let catalog = bundled_catalog();
        let rules = &catalog.get(TOU_MIRA_ID).unwrap().asset_rules;
        assert_eq!(
            latest_profile_version(
                &ReleaseHttp,
                TOU_MIRA_ID,
                rules,
                "x86",
                Store::Steam,
                Runtime::Native,
            )
            .unwrap(),
            "1.7.0"
        );
    }

    #[test]
    fn town_of_us_options_expose_only_the_target_specific_complete_zip() {
        let catalog = bundled_catalog();
        let rules = &catalog.get("AU-Avengers/TOU-Mira").unwrap().asset_rules;
        let release = resolver::parse_release(
            r#"{"tag_name":"1.6.3-beta2","assets":[{"name":"MiraAPI.dll","browser_download_url":"https://example.invalid/MiraAPI.dll","size":10},{"name":"TouMirav1.6.3b2-x64-epic-msstore.zip","browser_download_url":"https://example.invalid/x64.zip","size":30},{"name":"TouMirav1.6.3b2-x86-steam-itch.zip","browser_download_url":"https://example.invalid/x86.zip","size":31},{"name":"TouMirav1.6.3b2-x86-macOS-linux.zip","browser_download_url":"https://example.invalid/unix.zip","size":32},{"name":"TownOfUsMira.dll","browser_download_url":"https://example.invalid/TownOfUsMira.dll","size":20}]}"#,
        )
        .unwrap();

        for (arch, store, runtime, expected) in [
            (
                "x64",
                Store::Epic,
                Runtime::Native,
                "TouMirav1.6.3b2-x64-epic-msstore.zip",
            ),
            (
                "x86",
                Store::Steam,
                Runtime::Native,
                "TouMirav1.6.3b2-x86-steam-itch.zip",
            ),
            (
                "x86",
                Store::Steam,
                Runtime::Proton,
                "TouMirav1.6.3b2-x86-macOS-linux.zip",
            ),
        ] {
            let options = install_options_for_profile(
                vec![release.clone()],
                TOU_MIRA_ID,
                rules,
                arch,
                store,
                runtime,
            )
            .unwrap();
            assert_eq!(options.len(), 1);
            assert_eq!(options[0].asset_name, expected);
        }
        assert!(install_options_for_profile(
            vec![release],
            TOU_MIRA_ID,
            rules,
            "x86",
            Store::Epic,
            Runtime::Native,
        )
        .is_err());
    }

    #[test]
    fn catalog_repo_alias_reuses_and_unhides_canonical_entry() {
        let catalog = parse(
            r#"{"schema":1,"mods":[{"id":"Canonical/Mod","name":"Canonical","summary":"hosted","repo":"Alias/Repository","tags":[],"trust":"trusted","dependencies":[],"assetRules":{}}]}"#,
        )
        .unwrap();
        let entry = catalog_entry_for(&catalog, "Alias/Repository")
            .unwrap()
            .clone();
        let mut state = CatalogEnvelope {
            version: catalog_envelope_version(),
            order_policy_version: CATALOG_ORDER_POLICY_VERSION,
            display: vec![CatalogListItem {
                id: "Alias/Repository".into(),
                name: "Duplicate".into(),
                repo: "Alias/Repository".into(),
                summary: String::new(),
                tags: Vec::new(),
                dependencies: Vec::new(),
                provides: Vec::new(),
                dependency_versions: HashMap::new(),
                recommended_dependencies: Vec::new(),
                included: Vec::new(),
                latest: String::new(),
                trust: Trust::Flagged,
                extra: HashMap::new(),
            }],
            hosted_ids: vec![entry.id.clone()],
            hidden_hosted_ids: vec![entry.id.clone()],
            hosted_catalog: Some(catalog),
        };
        let effective_repo = entry.repo.clone().unwrap_or_else(|| entry.id.clone());

        ensure_display_catalog_state(
            &mut state,
            "Alias/Repository",
            &entry.id,
            &effective_repo,
            &entry.name,
            entry.summary,
            entry.tags,
        );

        assert!(state.hidden_hosted_ids.is_empty());
        assert_eq!(state.display.len(), 1);
        assert_eq!(state.display[0].id, "Canonical/Mod");
        assert_eq!(state.display[0].repo, "Alias/Repository");
    }

    #[test]
    fn hosted_reconciliation_keeps_custom_order_and_updates_hosted_entries() {
        let hosted = parse(
            r#"{"schema":1,"mods":[{"id":"Owner/Hosted","name":"New","summary":"fresh","repo":"Owner/Hosted","tags":[],"trust":"trusted","dependencies":[],"assetRules":{"perArch":{},"bundlesLoader":false}}]}"#,
        )
        .unwrap();
        let custom = CatalogListItem {
            id: "User/Custom".into(),
            name: "Custom".into(),
            repo: "User/Custom".into(),
            summary: String::new(),
            tags: Vec::new(),
            dependencies: Vec::new(),
            provides: Vec::new(),
            dependency_versions: HashMap::new(),
            recommended_dependencies: Vec::new(),
            included: Vec::new(),
            latest: String::new(),
            trust: Trust::Flagged,
            extra: HashMap::new(),
        };
        let old_hosted = CatalogListItem {
            id: "Owner/Hosted".into(),
            name: "Old".into(),
            repo: "Owner/Hosted".into(),
            summary: String::new(),
            tags: Vec::new(),
            dependencies: Vec::new(),
            provides: Vec::new(),
            dependency_versions: HashMap::new(),
            recommended_dependencies: Vec::new(),
            included: Vec::new(),
            latest: String::new(),
            trust: Trust::Flagged,
            extra: HashMap::new(),
        };
        let state = CatalogEnvelope {
            version: catalog_envelope_version(),
            order_policy_version: CATALOG_ORDER_POLICY_VERSION,
            display: vec![custom, old_hosted],
            hosted_ids: vec!["Owner/Hosted".into()],
            hidden_hosted_ids: Vec::new(),
            hosted_catalog: None,
        };
        let reconciled = reconcile_hosted(state, &hosted);
        assert_eq!(reconciled.display[0].id, "User/Custom");
        assert_eq!(reconciled.display[1].name, "New");
        assert_eq!(
            reconciled.hosted_catalog.as_ref().unwrap().mods[0].id,
            "Owner/Hosted"
        );
    }

    #[test]
    fn hosted_reconciliation_honors_tombstones_and_prunes_removed_provenance() {
        let hosted = parse(
            r#"{"schema":1,"mods":[{"id":"Owner/Hidden","name":"Hidden","summary":"","repo":"Owner/Hidden","tags":[],"trust":"trusted","dependencies":[],"assetRules":{}}]}"#,
        )
        .unwrap();
        let item = |id: &str| CatalogListItem {
            id: id.into(),
            name: id.into(),
            repo: id.into(),
            summary: String::new(),
            tags: Vec::new(),
            dependencies: Vec::new(),
            provides: Vec::new(),
            dependency_versions: HashMap::new(),
            recommended_dependencies: Vec::new(),
            included: Vec::new(),
            latest: String::new(),
            trust: Trust::Flagged,
            extra: HashMap::new(),
        };
        let state = CatalogEnvelope {
            version: catalog_envelope_version(),
            order_policy_version: CATALOG_ORDER_POLICY_VERSION,
            display: vec![
                item("User/Custom"),
                item("Owner/Hidden"),
                item("Owner/Gone"),
            ],
            hosted_ids: vec!["Owner/Hidden".into(), "Owner/Gone".into()],
            hidden_hosted_ids: vec!["Owner/Hidden".into(), "Owner/Gone".into()],
            hosted_catalog: None,
        };
        let reconciled = reconcile_hosted(state, &hosted);
        assert_eq!(
            reconciled
                .display
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            vec!["User/Custom"]
        );
        assert_eq!(reconciled.hidden_hosted_ids, vec!["Owner/Hidden"]);
        assert_eq!(reconciled.hosted_ids, vec!["Owner/Hidden"]);
    }

    #[test]
    fn recovers_a_registered_game_instance_after_its_folder_is_renamed() {
        let temp = tempfile::tempdir().unwrap();
        let original = temp.path().join("Among Us copy");
        let renamed = temp.path().join("Among Us copy renamed");
        fs::create_dir_all(&original).unwrap();
        let mut executable = vec![0_u8; 72];
        executable[0..2].copy_from_slice(b"MZ");
        executable[0x3c..0x40].copy_from_slice(&(64_u32).to_le_bytes());
        executable[64..68].copy_from_slice(b"PE\0\0");
        executable[68..70].copy_from_slice(&(0x014c_u16).to_le_bytes());
        fs::write(original.join(process::GAME_EXE), executable).unwrap();
        let identity = game_executable_identity(&original).unwrap();
        let mut saved = Settings::default();
        saved.game_instances.push(settings::GameInstance {
            id: "renamed".into(),
            name: "Renamed instance".into(),
            path: original.to_string_lossy().into_owned(),
            executable_identity: Some(identity.clone()),
            source_fingerprint: None,
            source_file_count: None,
            source_byte_count: None,
            arch: Arch::X86,
            store: Store::Manual,
            runtime: Runtime::Native,
            build: None,
            writable: true,
        });
        fs::rename(&original, &renamed).unwrap();

        let (changed, repaired) = repair_moved_game_instances(&mut saved).unwrap();

        assert!(changed);
        assert_eq!(repaired, vec!["Renamed instance"]);
        assert_eq!(
            PathBuf::from(&saved.game_instances[0].path),
            fs::canonicalize(&renamed).unwrap()
        );
        assert_eq!(
            saved.game_instances[0].executable_identity.as_deref(),
            Some(identity.as_str())
        );
    }

    #[test]
    fn repairs_store_for_an_existing_relocated_epic_instance() {
        let temp = tempfile::tempdir().unwrap();
        let game_dir = temp.path().join("Epic Games Games").join("AmongUs");
        fs::create_dir_all(&game_dir).unwrap();
        let mut executable = vec![0_u8; 72];
        executable[0..2].copy_from_slice(b"MZ");
        executable[0x3c..0x40].copy_from_slice(&(64_u32).to_le_bytes());
        executable[64..68].copy_from_slice(b"PE\0\0");
        executable[68..70].copy_from_slice(&(0x8664_u16).to_le_bytes());
        fs::write(game_dir.join(process::GAME_EXE), executable).unwrap();
        let mut saved = Settings::default();
        saved.game_instances.push(settings::GameInstance {
            id: "epic".into(),
            name: "Epic".into(),
            path: game_dir.to_string_lossy().into_owned(),
            executable_identity: None,
            source_fingerprint: None,
            source_file_count: None,
            source_byte_count: None,
            arch: Arch::X64,
            store: Store::Manual,
            runtime: Runtime::Native,
            build: None,
            writable: true,
        });

        let (changed, repaired) = repair_moved_game_instances(&mut saved).unwrap();

        assert!(changed);
        assert!(repaired.is_empty());
        assert_eq!(saved.game_instances[0].store, Store::Epic);
        assert!(saved.game_instances[0].executable_identity.is_some());
    }
    #[test]
    fn refreshes_registered_game_metadata_at_the_exact_saved_path() {
        let temporary = tempfile::tempdir().unwrap();
        let selected = temporary.path().join("selected");
        let stale_duplicate = temporary.path().join("stale duplicate");
        for (directory, machine, build) in [
            (&selected, 0x014c_u16, "2026.8.4"),
            (&stale_duplicate, 0x8664_u16, "2025.1.1"),
        ] {
            fs::create_dir_all(directory.join("Among Us_Data")).unwrap();
            let mut executable = vec![0_u8; 72];
            executable[0..2].copy_from_slice(b"MZ");
            executable[0x3c..0x40].copy_from_slice(&(64_u32).to_le_bytes());
            executable[64..68].copy_from_slice(b"PE\0\0");
            executable[68..70].copy_from_slice(&machine.to_le_bytes());
            fs::write(directory.join(process::GAME_EXE), executable).unwrap();
            fs::write(
                directory.join("Among Us_Data/globalgamemanagers"),
                format!("Among Us {build}"),
            )
            .unwrap();
        }
        let canonical = fs::canonicalize(&selected).unwrap();
        let expected_runtime = compat::resolve_with_hint(&canonical, Some(Runtime::Wine)).runtime;
        let mut saved = Settings::default();
        saved.game_instances.push(settings::GameInstance {
            id: "selected".into(),
            name: "Selected".into(),
            path: selected.to_string_lossy().into_owned(),
            executable_identity: Some("stale-fingerprint".into()),
            source_fingerprint: None,
            source_file_count: None,
            source_byte_count: None,
            arch: Arch::X64,
            store: Store::Manual,
            runtime: Runtime::Wine,
            build: Some("2024.1.1".into()),
            writable: false,
        });

        let (changed, repaired) = repair_moved_game_instances(&mut saved).unwrap();
        let refreshed = &saved.game_instances[0];

        assert!(changed);
        assert!(repaired.is_empty());
        assert_eq!(Path::new(&refreshed.path), canonical);
        assert_eq!(
            refreshed.executable_identity,
            game_executable_identity(&canonical)
        );
        assert_eq!(refreshed.arch, Arch::X86);
        assert_eq!(refreshed.runtime, expected_runtime);
        assert_eq!(refreshed.build.as_deref(), Some("2026.8.4"));
        assert!(refreshed.writable);
    }

    #[test]
    fn selected_game_folder_must_contain_the_executable() {
        let temp = tempfile::tempdir().unwrap();
        let error = validate_game_dir(temp.path()).unwrap_err();
        assert!(error.contains(process::GAME_EXE));
    }

    #[test]
    fn source_validation_leaves_no_probe() {
        let temp = tempfile::tempdir().unwrap();
        let mut executable = vec![0_u8; 72];
        executable[0..2].copy_from_slice(b"MZ");
        executable[0x3c..0x40].copy_from_slice(&(64_u32).to_le_bytes());
        executable[64..68].copy_from_slice(b"PE\0\0");
        executable[68..70].copy_from_slice(&(0x014c_u16).to_le_bytes());
        fs::write(temp.path().join(process::GAME_EXE), executable).unwrap();
        validate_game_dir(temp.path()).unwrap();
        assert!(fs::read_dir(temp.path()).unwrap().flatten().all(|entry| {
            !entry
                .file_name()
                .to_string_lossy()
                .starts_with(".perfectsync-write-test-")
        }));
    }

    #[test]
    fn recursive_game_copy_preserves_tree_and_skips_internal_work_files() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let destination = temp.path().join("managed");
        fs::create_dir_all(source.join("Among Us_Data")).unwrap();
        fs::write(source.join(process::GAME_EXE), b"MZ").unwrap();
        fs::write(
            source.join("Among Us_Data/globalgamemanagers"),
            b"game-data",
        )
        .unwrap();
        fs::write(source.join(".perfectsync-stage-stale"), b"stale").unwrap();
        let mut files = 0;
        let mut bytes = 0;

        copy_game_tree(&source, &destination, &mut files, &mut bytes).unwrap();

        assert_eq!(files, 2);
        assert_eq!(bytes, 11);
        assert_eq!(
            fs::read(destination.join(process::GAME_EXE)).unwrap(),
            b"MZ"
        );
        assert_eq!(
            fs::read(destination.join("Among Us_Data/globalgamemanagers")).unwrap(),
            b"game-data"
        );
        assert!(!destination.join(".perfectsync-stage-stale").exists());
    }
    #[test]
    fn nonexistent_profile_loader_preflight_creates_no_layout_or_record() {
        use std::cell::Cell;

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("profiles");
        let ran = Cell::new(false);
        let result = with_existing_profile_layout(&root, "missing", || {
            ran.set(true);
            Ok(())
        });

        assert_eq!(result.unwrap_err(), "profile not found");
        assert!(!ran.get());
        assert!(ProfileStore::new(&root).list().unwrap().is_empty());
        assert!(!root.join("missing").exists());
    }

    #[test]
    fn pinned_loader_selects_exact_build_753_archive_for_each_arch() {
        assert_eq!(
            pinned_loader("x86").unwrap(),
            (
                PINNED_LOADER_VERSION.to_string(),
                PINNED_LOADER_X86_URL.to_string()
            )
        );
        assert_eq!(
            pinned_loader("x64").unwrap(),
            (
                PINNED_LOADER_VERSION.to_string(),
                PINNED_LOADER_X64_URL.to_string()
            )
        );
        assert!(pinned_loader("arm64").is_err());
    }

    #[test]
    fn hosted_only_dependency_is_not_authoritative() {
        let hosted = parse(
            r#"{"schema":1,"mods":[
                {"id":"Owner/Root","name":"Root","summary":"","repo":"Owner/Root","tags":[],"trust":"trusted","dependencies":["Attacker/Dependency"],"assetRules":{}},
                {"id":"Attacker/Dependency","name":"Dependency","summary":"","repo":"Attacker/Dependency","tags":[],"trust":"trusted","dependencies":[],"assetRules":{}}
            ]}"#,
        )
        .unwrap();

        let error =
            validate_authoritative_dependencies(&hosted, &["Owner/Root".into()]).unwrap_err();
        assert!(error.contains("not authorized by the bundled catalog"));
    }

    #[test]
    fn bundled_identity_cannot_be_reused_by_an_explicit_root_with_another_repo() {
        let custom = parse(
            r#"{"schema":1,"mods":[
                {"id":"Owner/Manual","name":"Manual","summary":"","repo":"Owner/Manual","tags":[],"trust":"flagged","dependencies":[],"assetRules":{}}
            ]}"#,
        )
        .unwrap();
        validate_authoritative_dependencies(&custom, &["Owner/Manual".into()]).unwrap();

        let impersonated = parse(
            r#"{"schema":1,"mods":[
                {"id":"NuclearPowered/Reactor","name":"Reactor","summary":"","repo":"Attacker/Reactor","tags":[],"trust":"trusted","dependencies":[],"assetRules":{}}
            ]}"#,
        )
        .unwrap();
        let error =
            validate_authoritative_dependencies(&impersonated, &["NuclearPowered/Reactor".into()])
                .unwrap_err();
        assert!(error.contains("Catalog root NuclearPowered/Reactor"));
        assert!(error.contains("bundled authoritative identity"));
    }

    #[test]
    fn installed_bundle_provider_skips_separate_dependency_downloads() {
        struct NoNetwork;

        impl Http for NoNetwork {
            fn get_text(
                &self,
                _url: &str,
            ) -> Result<String, perfect_sync_core::resolver::ResolveError> {
                panic!("an installed bundle provider must not fetch release metadata")
            }

            fn get_bytes(
                &self,
                _url: &str,
            ) -> Result<Vec<u8>, perfect_sync_core::resolver::ResolveError> {
                panic!("an installed bundle provider must not download a dependency")
            }
        }

        let temp = tempfile::tempdir().unwrap();
        let profile_id = "cached-tou";
        let package = epic_zip(&[
            ("BepInEx/plugins/Mini.RegionInstall.dll", b"mini"),
            ("BepInEx/plugins/MiraAPI.dll", b"mira"),
            ("BepInEx/plugins/Reactor.dll", b"reactor"),
            ("BepInEx/plugins/touhats.bundle", b"bundle"),
            ("BepInEx/plugins/touhats.catalog", b"catalog"),
            ("BepInEx/plugins/TownOfUsMira.dll", b"tou"),
            ("BepInEx/config/at.duikbo.regioninstall.cfg", b"config"),
        ]);
        profile::install_tou_bundle_zip_bytes(temp.path(), profile_id, &package).unwrap();
        let mut record = ProfileRecord {
            id: profile_id.into(),
            name: "Cached Town of Us".into(),
            crew_color: "#fff".into(),
            game_build: None,
            game_instance_id: None,
            mods: vec![InstalledMod {
                package_id: TOU_MIRA_ID.into(),
                name: "Town of Us - Mira".into(),
                repo: Some(TOU_MIRA_ID.into()),
                version: "1.6.3-beta2".into(),
                versions: vec!["1.6.3-beta2".into()],
                enabled: true,
                source: ModSource::Github,
                tags: vec![ModTag::Role, ModTag::AllClient],
                managed: false,
                update: None,
                file: Some("TownOfUsMira.dll".into()),
                asset: Some("TouMirav1.6.3b2-x86-steam-itch.zip".into()),
            }],
            levelimposter_maps: Vec::new(),
        };
        let catalog = bundled_catalog();
        let context = InstallContext {
            stage_root: temp.path(),
            profile_id,
            http: &NoNetwork,
            catalog: &catalog,
            arch: "x86",
            store: Store::Steam,
            runtime: Runtime::Native,
        };

        install_catalog_latest(&context, &mut record, TOU_MIRA_ID, true, &[]).unwrap();
        install_catalog_latest(&context, &mut record, "NuclearPowered/Reactor", true, &[]).unwrap();

        assert_eq!(record.mods.len(), 1);
        assert!(!record.mods[0].managed);
        assert_eq!(
            catalog
                .get(TOU_MIRA_ID)
                .unwrap()
                .provides
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            TOU_BUNDLED_DEPENDENCY_IDS
        );
    }
    #[test]
    fn provider_resolution_is_catalog_driven_and_requires_an_enabled_bundle() {
        let catalog = parse(
            r#"{"schema":1,"mods":[
                {"id":"Owner/Bundle","name":"Complete Bundle","summary":"","repo":"Owner/Bundle","tags":[],"trust":"trusted","dependencies":[],"provides":["Shared/Api"],"assetRules":{}},
                {"id":"Shared/Api","name":"Shared API","summary":"","repo":"Shared/Api","tags":[],"trust":"trusted","dependencies":[],"assetRules":{}}
            ]}"#,
        )
        .unwrap();
        let mut record = ProfileRecord {
            id: "generic-provider".into(),
            name: "Generic provider".into(),
            crew_color: "#fff".into(),
            game_build: None,
            game_instance_id: None,
            mods: vec![InstalledMod {
                package_id: "Owner/Bundle".into(),
                name: "Complete Bundle".into(),
                repo: Some("Owner/Bundle".into()),
                version: "v1".into(),
                versions: vec!["v1".into()],
                enabled: true,
                source: ModSource::Github,
                tags: Vec::new(),
                managed: false,
                update: None,
                file: Some("Bundle.dll".into()),
                asset: Some("Bundle.zip".into()),
            }],
            levelimposter_maps: Vec::new(),
        };
        let context = InstallContext {
            stage_root: Path::new("."),
            profile_id: "generic-provider",
            http: &DownloadHttp(b""),
            catalog: &catalog,
            arch: "x86",
            store: Store::Steam,
            runtime: Runtime::Native,
        };

        assert!(reuse_installed_dependency(&context, &mut record, "Shared/Api", &[]).unwrap());
        record.mods[0].enabled = false;
        assert!(!reuse_installed_dependency(&context, &mut record, "Shared/Api", &[]).unwrap());
    }

    #[test]
    fn default_catalog_order_prioritizes_major_mods_and_puts_dependencies_last() {
        let catalog = bundled_catalog();
        let mut display: Vec<_> = catalog.mods.into_iter().map(catalog_item).collect();
        apply_default_catalog_order(&mut display);

        assert_eq!(
            display
                .iter()
                .take(PRIORITY_CATALOG_IDS.len())
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            PRIORITY_CATALOG_IDS
        );
        let dependency_start = display
            .iter()
            .position(|item| catalog_display_rank(item) == usize::MAX)
            .unwrap();
        assert!(display[dependency_start..]
            .iter()
            .all(|item| catalog_display_rank(item) == usize::MAX));
    }

    #[test]
    fn town_of_us_extensions_recommend_town_of_us_without_auto_installing_it() {
        let catalog = bundled_catalog();
        for id in [
            "DivaniNL/TownOfUsMiraDivaniModsAddOn",
            "Mehzxzz/TownOfExtra",
            "rewalo/TownOfUsMiraRolesExtension",
            "idkimneil/DraftMode-TOUM",
        ] {
            let entry = catalog.get(id).unwrap();
            assert!(entry.dependencies.is_empty(), "{id}");
            assert_eq!(
                entry.recommended_dependencies,
                vec![TOU_MIRA_ID.to_string()],
                "{id}"
            );
        }
    }

    #[test]
    fn bundled_dependency_cannot_be_removed_while_town_of_us_is_installed() {
        let temp = tempfile::tempdir().unwrap();
        let profile_id = "managed-tou-dependency";
        let installed = |id: &str, file: &str| InstalledMod {
            package_id: id.into(),
            name: id.into(),
            repo: Some(id.into()),
            version: "v1".into(),
            versions: vec!["v1".into()],
            enabled: true,
            source: ModSource::Github,
            tags: Vec::new(),
            managed: false,
            update: None,
            file: Some(file.into()),
            asset: Some(file.into()),
        };
        profile::install_plugin_bytes(temp.path(), profile_id, "Reactor.dll", b"reactor").unwrap();
        let mut record = ProfileRecord {
            id: profile_id.into(),
            name: "Managed Town of Us dependency".into(),
            crew_color: "#fff".into(),
            game_build: None,
            game_instance_id: None,
            mods: vec![
                installed("AU-Avengers/TOU-Mira", "Tou.dll"),
                InstalledMod {
                    managed: true,
                    ..installed("NuclearPowered/Reactor", "Reactor.dll")
                },
            ],
            levelimposter_maps: Vec::new(),
        };
        let catalog = bundled_catalog();

        let error = remove_mod_from_record(
            temp.path(),
            profile_id,
            &mut record,
            &catalog,
            "NuclearPowered/Reactor",
        )
        .unwrap_err();

        assert!(error.contains("supplied by the enabled AU-Avengers/TOU-Mira bundle"));
        assert!(record
            .mods
            .iter()
            .any(|installed| installed.package_id == "NuclearPowered/Reactor"));
        assert!(temp
            .path()
            .join(profile_id)
            .join("BepInEx/plugins/Reactor.dll")
            .is_file());
    }

    #[test]
    fn operation_progress_channel_uses_frontend_contract() {
        let (sender, receiver) = std::sync::mpsc::channel::<serde_json::Value>();
        let channel = Channel::new(move |body| {
            sender
                .send(body.deserialize().unwrap())
                .expect("progress receiver must remain connected");
            Ok(())
        });
        let reporter = ProgressReporter::new(channel);

        reporter.stage("preparing", "Checking profile");
        reporter.download("Downloading Reactor.dll", 64, Some(128));

        let events: Vec<_> = receiver.try_iter().collect();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["phase"], "preparing");
        assert_eq!(events[0]["message"], "Checking profile");
        assert!(events[0].get("bytesReceived").is_none());
        assert_eq!(events[1]["phase"], "downloading");
        assert_eq!(events[1]["bytesReceived"], 64);
        assert_eq!(events[1]["bytesTotal"], 128);
    }

    #[test]
    fn epic_auth_store_seeds_login_without_overwriting_tokens() {
        let temp = tempfile::tempdir().unwrap();
        let token_store = ensure_epic_auth_store(temp.path()).unwrap();
        assert_eq!(token_store, temp.path().join(".config/legendary/user.json"));
        assert_eq!(fs::read(&token_store).unwrap(), b"null\n");

        fs::write(&token_store, b"existing session").unwrap();
        assert_eq!(ensure_epic_auth_store(temp.path()).unwrap(), token_store);
        assert_eq!(fs::read(token_store).unwrap(), b"existing session");
    }

    #[test]
    fn epic_auth_store_covers_linux_and_macos_wine_profiles() {
        for (name, host, runtime) in [
            ("linux", compat::HostPlatform::Linux, Runtime::Wine),
            ("macos", compat::HostPlatform::Macos, Runtime::Whisky),
        ] {
            let temp = tempfile::tempdir().unwrap();
            let game_dir = temp.path().join(format!("{name}-game"));
            let prefix = temp.path().join(format!("{name}-prefix"));
            let user_profile = prefix.join("drive_c/users/steamuser");
            fs::create_dir_all(user_profile.join("AppData/LocalLow/Innersloth/Among Us")).unwrap();
            fs::create_dir(&game_dir).unwrap();
            let context = compat::RuntimeContext {
                host,
                runtime,
                prefix: Some(prefix),
                launcher: None,
                launcher_args: Vec::new(),
            };

            prepare_epic_auth_stores(&game_dir, &context).unwrap();

            assert_eq!(
                fs::read(user_profile.join(".config/legendary/user.json")).unwrap(),
                b"null\n"
            );
            assert!(!game_dir.join("EGSAuth.json").exists());
        }
    }

    #[test]
    fn epic_auth_store_falls_back_to_game_folder_without_a_wine_profile() {
        let temp = tempfile::tempdir().unwrap();
        let game_dir = temp.path().join("game");
        fs::create_dir(&game_dir).unwrap();
        let context = compat::RuntimeContext {
            host: compat::HostPlatform::Other,
            runtime: Runtime::Wine,
            prefix: None,
            launcher: None,
            launcher_args: Vec::new(),
        };

        prepare_epic_auth_stores(&game_dir, &context).unwrap();

        assert_eq!(fs::read(game_dir.join("EGSAuth.json")).unwrap(), b"null\n");
    }

    #[test]
    fn manual_install_requires_confirmation_before_work_starts() {
        assert!(require_manual_install_confirmation(true).is_ok());
        let error = require_manual_install_confirmation(false).unwrap_err();
        assert!(error.contains("repository, release tag, and asset"));
    }

    #[test]
    fn durable_profile_mutations_refresh_compatible_source_changes() {
        const CHILD: &str = "PERFECT_SYNC_MUTATION_SOURCE_CHILD";
        const ROOT: &str = "PERFECT_SYNC_MUTATION_SOURCE_ROOT";
        const TEST: &str =
            "commands::tests::durable_profile_mutations_refresh_compatible_source_changes";

        if std::env::var_os(CHILD).is_some() {
            let root = PathBuf::from(std::env::var_os(ROOT).unwrap());
            settings::initialize_app_data_dir(root.join("app")).unwrap();
            settings::initialize_managed_data_dir(root.join("managed")).unwrap();
            let source = root.join("source");
            fs::create_dir_all(source.join("Among Us_Data")).unwrap();
            let mut executable = vec![0_u8; 72];
            executable[0..2].copy_from_slice(b"MZ");
            executable[0x3c..0x40].copy_from_slice(&(64_u32).to_le_bytes());
            executable[64..68].copy_from_slice(b"PE\0\0");
            executable[68..70].copy_from_slice(&(0x014c_u16).to_le_bytes());
            fs::write(source.join("Among Us.exe"), executable).unwrap();
            fs::write(source.join("Among Us_Data/data.unity3d"), b"game data").unwrap();
            fs::write(
                source.join("Among Us_Data/globalgamemanagers"),
                b"Among Us 2026.8.4",
            )
            .unwrap();
            let source = fs::canonicalize(source).unwrap();
            let mut instance = settings::GameInstance {
                id: "source-1".into(),
                name: "Source".into(),
                path: source.to_string_lossy().into_owned(),
                executable_identity: None,
                source_fingerprint: None,
                source_file_count: None,
                source_byte_count: None,
                arch: Arch::X86,
                store: Store::Steam,
                runtime: Runtime::Native,
                build: Some("2026.8.4".into()),
                writable: true,
            };
            let managed = managed_instance::record_source(&instance).unwrap();
            let original_fingerprint = managed.record.fingerprint.clone();
            instance.source_fingerprint = Some(managed.record.fingerprint.clone());
            instance.source_file_count = Some(managed.record.file_count);
            instance.source_byte_count = Some(managed.record.byte_count);
            settings::save(&Settings {
                game_instances: vec![instance],
                active_profile: Some("profile-1".into()),
                skip_launch_warning: true,
                ..Settings::default()
            })
            .unwrap();
            ProfileStore::new(settings::profiles_root())
                .save(&ProfileRecord {
                    id: "profile-1".into(),
                    name: "Profile".into(),
                    crew_color: "#fff".into(),
                    game_build: Some("2026.8.4".into()),
                    game_instance_id: Some("source-1".into()),
                    mods: Vec::new(),
                    levelimposter_maps: Vec::new(),
                })
                .unwrap();

            require_profile_source_for_install("profile-1").unwrap();
            fs::write(source.join("Among Us_Data/data.unity3d"), b"changed").unwrap();
            require_profile_source_for_install("profile-1").unwrap();
            let refreshed = settings::load().unwrap();
            let refreshed_instance = &refreshed.game_instances[0];
            assert_ne!(
                refreshed_instance.source_fingerprint.as_deref(),
                Some(original_fingerprint.as_str())
            );
            assert_eq!(refreshed.active_profile.as_deref(), Some("profile-1"));
            assert!(refreshed.skip_launch_warning);
            let refreshed_source = managed_instance::saved_source(refreshed_instance)
                .unwrap()
                .unwrap();
            managed_instance::ensure_exact_source_available(&refreshed_source).unwrap();

            fs::write(
                source.join("Among Us_Data/globalgamemanagers"),
                b"Among Us 2026.9.1",
            )
            .unwrap();
            assert!(require_profile_source_for_install("profile-1")
                .unwrap_err()
                .contains("requires build 2026.8.4"));
            fs::remove_dir_all(&source).unwrap();
            assert!(require_profile_source_for_install("profile-1")
                .unwrap_err()
                .contains("game folder not found"));
            return;
        }

        let root = tempfile::tempdir().unwrap();
        let output = process::command(std::env::current_exe().unwrap())
            .env(CHILD, "1")
            .env(ROOT, root.path())
            .args(["--exact", TEST, "--nocapture"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn crossover_allows_five_minutes_for_cold_bottle_startup() {
        assert_eq!(CROSSOVER_GAME_START_TIMEOUT, Duration::from_secs(5 * 60));
    }

    #[test]
    fn crossover_supervisor_waits_for_delayed_exact_game_process() {
        const ROLE: &str = "PERFECT_SYNC_CROSSOVER_SUPERVISOR_ROLE";
        const TEST: &str =
            "commands::tests::crossover_supervisor_waits_for_delayed_exact_game_process";

        if let Ok(role) = std::env::var(ROLE) {
            match role.as_str() {
                "launcher" => std::thread::sleep(Duration::from_secs(5)),
                "game" => std::thread::sleep(Duration::from_secs(5)),
                _ => panic!("unexpected CrossOver supervisor role"),
            }
            return;
        }

        let temp = tempfile::tempdir().unwrap();
        let game_dir = temp.path().join("managed-game");
        fs::create_dir(&game_dir).unwrap();
        let game_executable = game_dir.join(process::GAME_EXE);
        fs::copy(std::env::current_exe().unwrap(), &game_executable).unwrap();

        let launcher = process::command(std::env::current_exe().unwrap())
            .env(ROLE, "launcher")
            .args(["--exact", TEST, "--nocapture"])
            .spawn()
            .unwrap();
        let delayed_game = std::thread::spawn({
            let game_executable = game_executable.clone();
            move || {
                std::thread::sleep(Duration::from_millis(200));
                process::command(game_executable)
                    .env(ROLE, "game")
                    .args(["--exact", TEST, "--nocapture"])
                    .spawn()
                    .unwrap()
            }
        });

        supervise_crossover_launch(launcher, &game_dir, Duration::from_secs(10)).unwrap();
        let mut game = delayed_game.join().unwrap();
        assert!(game.wait().unwrap().success());
    }

    #[test]
    fn crossover_supervisor_rejects_a_transient_exact_game_process() {
        const ROLE: &str = "PERFECT_SYNC_CROSSOVER_TRANSIENT_ROLE";
        const TEST: &str =
            "commands::tests::crossover_supervisor_rejects_a_transient_exact_game_process";

        if let Ok(role) = std::env::var(ROLE) {
            match role.as_str() {
                "launcher" => std::thread::sleep(Duration::from_secs(5)),
                "game" => std::thread::sleep(Duration::from_secs(1)),
                _ => panic!("unexpected transient CrossOver role"),
            }
            return;
        }

        let temp = tempfile::tempdir().unwrap();
        let game_dir = temp.path().join("managed-game");
        fs::create_dir(&game_dir).unwrap();
        let game_executable = game_dir.join(process::GAME_EXE);
        fs::copy(std::env::current_exe().unwrap(), &game_executable).unwrap();
        let launcher = process::command(std::env::current_exe().unwrap())
            .env(ROLE, "launcher")
            .args(["--exact", TEST, "--nocapture"])
            .spawn()
            .unwrap();
        let delayed_game = std::thread::spawn({
            let game_executable = game_executable.clone();
            move || {
                std::thread::sleep(Duration::from_millis(200));
                process::command(game_executable)
                    .env(ROLE, "game")
                    .args(["--exact", TEST, "--nocapture"])
                    .spawn()
                    .unwrap()
            }
        });

        let error =
            supervise_crossover_launch(launcher, &game_dir, Duration::from_secs(5)).unwrap_err();
        assert!(error.contains("Among Us exited before remaining alive for 3 seconds"));
        let mut game = delayed_game.join().unwrap();
        assert!(game.wait().unwrap().success());
    }

    #[test]
    fn crossover_supervisor_releases_interactive_helper_after_readiness() {
        const ROLE: &str = "PERFECT_SYNC_CROSSOVER_INTERACTIVE_ROLE";
        const MARKER: &str = "PERFECT_SYNC_CROSSOVER_INTERACTIVE_MARKER";
        const TEST: &str =
            "commands::tests::crossover_supervisor_releases_interactive_helper_after_readiness";

        if let Ok(role) = std::env::var(ROLE) {
            match role.as_str() {
                "helper" => {
                    let mut input = String::new();
                    io::stdin().read_line(&mut input).unwrap();
                    assert_eq!(input, "\n");
                    fs::write(std::env::var_os(MARKER).unwrap(), b"released").unwrap();
                }
                "game" => std::thread::sleep(Duration::from_secs(5)),
                _ => panic!("unexpected CrossOver interactive role"),
            }
            return;
        }

        let temp = tempfile::tempdir().unwrap();
        let game_dir = temp.path().join("managed-game");
        fs::create_dir(&game_dir).unwrap();
        let game_executable = game_dir.join(process::GAME_EXE);
        fs::copy(std::env::current_exe().unwrap(), &game_executable).unwrap();
        let marker = temp.path().join("helper-released");
        let specification = process::LaunchSpec {
            program: std::env::current_exe().unwrap(),
            args: vec!["--exact".into(), TEST.into(), "--nocapture".into()],
            cwd: std::env::current_dir().unwrap(),
            env: vec![
                (ROLE.into(), "helper".into()),
                (MARKER.into(), marker.as_os_str().to_owned()),
            ],
            error: None,
        };
        let launcher = launch_crossover(&specification, true).unwrap();
        let delayed_game = std::thread::spawn({
            let game_executable = game_executable.clone();
            move || {
                std::thread::sleep(Duration::from_millis(200));
                process::command(game_executable)
                    .env(ROLE, "game")
                    .args(["--exact", TEST, "--nocapture"])
                    .spawn()
                    .unwrap()
            }
        });

        supervise_crossover_launch(launcher, &game_dir, Duration::from_secs(10)).unwrap();
        let marker_deadline = Instant::now() + Duration::from_secs(2);
        while !marker.exists() && Instant::now() < marker_deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(fs::read(marker).unwrap(), b"released");
        let mut game = delayed_game.join().unwrap();
        assert!(game.wait().unwrap().success());
    }

    #[test]
    fn crossover_supervisor_reports_immediate_wrapper_failure_output() {
        const CHILD: &str = "PERFECT_SYNC_CROSSOVER_FAILURE_CHILD";
        const TEST: &str =
            "commands::tests::crossover_supervisor_reports_immediate_wrapper_failure_output";

        if std::env::var_os(CHILD).is_some() {
            println!("wrapper dispatch detail");
            io::stdout().flush().unwrap();
            eprint!("{}", "x".repeat(CROSSOVER_OUTPUT_BYTES_PER_STREAM * 2));
            eprintln!("bottle configuration missing");
            io::stderr().flush().unwrap();
            std::process::exit(23);
        }

        let temp = tempfile::tempdir().unwrap();
        let game_dir = temp.path().join("managed-game");
        fs::create_dir(&game_dir).unwrap();
        let specification = process::LaunchSpec {
            program: std::env::current_exe().unwrap(),
            args: vec!["--exact".into(), TEST.into(), "--nocapture".into()],
            cwd: std::env::current_dir().unwrap(),
            env: vec![(CHILD.into(), "1".into())],
            error: None,
        };
        let launcher = launch_crossover(&specification, false).unwrap();

        let started = Instant::now();
        let error =
            supervise_crossover_launch(launcher, &game_dir, Duration::from_secs(3)).unwrap_err();
        assert!(error.contains("attached wrapper exited with"));
        assert!(error.contains("23"));
        assert!(error.contains("before the exact managed Among Us process became ready"));
        assert!(error.contains("CrossOver stdout"));
        assert!(error.contains("wrapper dispatch detail"));
        assert!(error.contains("CrossOver stderr"));
        assert!(error.contains("earlier output omitted"));
        assert!(error.contains("bottle configuration missing"));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn crossover_supervisor_reports_live_wrapper_and_output_at_timeout() {
        const CHILD: &str = "PERFECT_SYNC_CROSSOVER_TIMEOUT_CHILD";
        const TEST: &str =
            "commands::tests::crossover_supervisor_reports_live_wrapper_and_output_at_timeout";

        if std::env::var_os(CHILD).is_some() {
            eprintln!("wrapper remained attached");
            io::stderr().flush().unwrap();
            std::thread::sleep(Duration::from_secs(5));
            return;
        }

        let temp = tempfile::tempdir().unwrap();
        let game_dir = temp.path().join("managed-game");
        fs::create_dir(&game_dir).unwrap();
        let specification = process::LaunchSpec {
            program: std::env::current_exe().unwrap(),
            args: vec!["--exact".into(), TEST.into(), "--nocapture".into()],
            cwd: std::env::current_dir().unwrap(),
            env: vec![(CHILD.into(), "1".into())],
            error: None,
        };
        let launcher = launch_crossover(&specification, false).unwrap();

        let started = Instant::now();
        let error =
            supervise_crossover_launch(launcher, &game_dir, Duration::from_secs(1)).unwrap_err();
        assert!(error.contains("within 1 second"));
        assert!(error.contains("attached wrapper remained alive"));
        assert!(error.contains("was stopped"));
        assert!(error.contains("launch is no longer pending"));
        assert!(error.contains("CrossOver stderr"));
        assert!(error.contains("wrapper remained attached"));
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn crossover_failures_clear_pending_for_repeat_launch() {
        const CHILD: &str = "PERFECT_SYNC_REPEAT_LAUNCH_CHILD";
        const ROOT: &str = "PERFECT_SYNC_REPEAT_LAUNCH_ROOT";
        const TEST: &str = "commands::tests::crossover_failures_clear_pending_for_repeat_launch";

        if std::env::var_os(CHILD).is_some() {
            settings::initialize_managed_data_dir(PathBuf::from(std::env::var_os(ROOT).unwrap()))
                .unwrap();
            let game_dir = managed_instance::workspace_game_dir("repeat-profile").unwrap();
            fs::create_dir_all(&game_dir).unwrap();

            assert_eq!(
                spawn_launch("repeat-profile", || {
                    wait_for_crossover_process(
                        &game_dir.join(process::GAME_EXE),
                        "Among Us",
                        Duration::from_secs(1),
                        Duration::ZERO,
                        || Err("dispatch failed".into()),
                    )
                    .map(|_| ())
                })
                .unwrap_err(),
                "dispatch failed"
            );
            assert!(!launch_pending("repeat-profile").unwrap());

            assert_eq!(
                spawn_launch("repeat-profile", || {
                    assert!(launch_pending("repeat-profile").unwrap());
                    if wait_for_crossover_process(
                        &game_dir.join(process::GAME_EXE),
                        "Among Us",
                        Duration::ZERO,
                        Duration::ZERO,
                        || Ok(()),
                    )? {
                        Ok(())
                    } else {
                        Err("game start timed out".into())
                    }
                })
                .unwrap_err(),
                "game start timed out"
            );
            assert!(!launch_pending("repeat-profile").unwrap());

            assert_eq!(
                spawn_launch("repeat-profile", || Err("retry reached dispatcher".into()))
                    .unwrap_err(),
                "retry reached dispatcher"
            );
            assert!(!launch_pending("repeat-profile").unwrap());
            return;
        }

        let root = tempfile::tempdir().unwrap();
        let output = process::command(std::env::current_exe().unwrap())
            .env(CHILD, "1")
            .env(ROOT, root.path())
            .args(["--exact", TEST, "--nocapture"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn concurrent_profile_sessions_are_independent() {
        const COORDINATOR: &str = "PERFECT_SYNC_CONCURRENT_COORDINATOR";
        const FAKE_GAME: &str = "PERFECT_SYNC_CONCURRENT_FAKE_GAME";
        const MANAGED_ROOT: &str = "PERFECT_SYNC_CONCURRENT_MANAGED_ROOT";
        const TEST_NAME: &str = "commands::tests::concurrent_profile_sessions_are_independent";

        if std::env::var_os(FAKE_GAME).is_some() {
            std::thread::sleep(Duration::from_secs(5));
            return;
        }
        if std::env::var_os(COORDINATOR).is_some() {
            let managed_root = PathBuf::from(std::env::var_os(MANAGED_ROOT).unwrap());
            settings::initialize_managed_data_dir(managed_root).unwrap();
            let test_executable = std::env::current_exe().unwrap();
            let steam_dir = managed_instance::workspace_game_dir("steam-profile").unwrap();
            let epic_dir = managed_instance::workspace_game_dir("epic-profile").unwrap();
            fs::create_dir_all(&steam_dir).unwrap();
            fs::create_dir_all(&epic_dir).unwrap();
            let steam_executable = steam_dir.join(process::GAME_EXE);
            let epic_executable = epic_dir.join(process::GAME_EXE);
            fs::copy(&test_executable, &steam_executable).unwrap();
            fs::copy(&test_executable, &epic_executable).unwrap();

            let spawn_fake = |executable: &Path| {
                process::command(executable)
                    .env(FAKE_GAME, "1")
                    .args(["--exact", TEST_NAME])
                    .spawn()
                    .map_err(|error| error.to_string())
            };
            let mut steam = None;
            spawn_launch("steam-profile", || {
                steam = Some(spawn_fake(&steam_executable)?);
                Ok(())
            })
            .unwrap();
            let mut epic = None;
            spawn_launch("epic-profile", || {
                epic = Some(spawn_fake(&epic_executable)?);
                Ok(())
            })
            .unwrap();

            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                if process::try_is_game_dir_running(&steam_dir).unwrap()
                    && process::try_is_game_dir_running(&epic_dir).unwrap()
                    && !launch_pending("steam-profile").unwrap()
                    && !launch_pending("epic-profile").unwrap()
                {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "both profile sessions did not become independently visible"
                );
                std::thread::sleep(Duration::from_millis(25));
            }
            assert!(workspace_is_stopped("steam-profile").is_err());
            assert!(workspace_is_stopped("epic-profile").is_err());

            let mut steam = steam.unwrap();
            steam.kill().unwrap();
            steam.wait().unwrap();
            assert!(workspace_is_stopped("steam-profile").is_ok());
            assert!(workspace_is_stopped("epic-profile").is_err());

            let mut epic = epic.unwrap();
            epic.kill().unwrap();
            epic.wait().unwrap();
            assert!(workspace_is_stopped("epic-profile").is_ok());
            return;
        }

        let managed_root = tempfile::tempdir().unwrap();
        let output = process::command(std::env::current_exe().unwrap())
            .env(COORDINATOR, "1")
            .env(MANAGED_ROOT, managed_root.path())
            .args(["--exact", TEST_NAME, "--nocapture"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    #[ignore = "requires a local Among Us source and profile fixture"]
    fn live_managed_workspace_smoke() {
        let app_data = PathBuf::from(std::env::var("PERFECT_SYNC_SMOKE_APP_DATA").unwrap());
        let managed_data = PathBuf::from(std::env::var("PERFECT_SYNC_SMOKE_MANAGED_DATA").unwrap());
        let profile_id = std::env::var("PERFECT_SYNC_SMOKE_PROFILE").unwrap();
        let source = std::env::var("PERFECT_SYNC_SMOKE_SOURCE").unwrap();
        settings::initialize_app_data_dir(app_data).unwrap();
        settings::initialize_managed_data_dir(managed_data).unwrap();

        let source_mira = Path::new(&source).join("BepInEx/plugins/MiraAPI.dll");
        let source_mira_before = fs::read(&source_mira).ok();
        prepare_profile_with_guard(&source, &profile_id, None, true, || Ok(())).unwrap();

        let marker = managed_instance::active_marker(&profile_id)
            .unwrap()
            .unwrap();
        let active = managed_instance::workspace_game_dir(&profile_id).unwrap();
        assert_eq!(marker.profile_id, profile_id);
        assert_ne!(
            fs::canonicalize(&active).unwrap(),
            fs::canonicalize(&source).unwrap()
        );
        assert!(loader::has_loader(&active));
        validate_managed_launch_target(&active, &profile_id).unwrap();
        assert!(validate_managed_launch_target(Path::new(&source), &profile_id).is_err());

        let profile_record = recovered_profile_store(&settings::profiles_root())
            .unwrap()
            .load(&profile_id)
            .unwrap()
            .unwrap();
        if let Some(installed) = profile_record
            .mods
            .iter()
            .find(|installed| installed.enabled && is_tou_mira(&installed.package_id))
        {
            let saved = settings::load().unwrap();
            let instance = saved
                .game_instances
                .iter()
                .find(|instance| instance.id == marker.game_instance_id)
                .unwrap();
            let arch = arch_str(instance.arch);
            let package =
                load_tou_package_bytes(installed, &arch, instance.store, instance.runtime).unwrap();
            let mut archive = zip::ZipArchive::new(Cursor::new(package)).unwrap();
            for name in ["MiraAPI.dll", "touhats.bundle", "touhats.catalog"] {
                let mut expected = Vec::new();
                let mut found = false;
                for index in 0..archive.len() {
                    let mut entry = archive.by_index(index).unwrap();
                    if entry
                        .name()
                        .rsplit('/')
                        .next()
                        .is_some_and(|entry_name| entry_name.eq_ignore_ascii_case(name))
                    {
                        entry.read_to_end(&mut expected).unwrap();
                        found = true;
                        break;
                    }
                }
                assert!(found, "{name} missing from release package");
                assert_eq!(
                    fs::read(active.join("BepInEx").join("plugins").join(name)).unwrap(),
                    expected,
                    "{name} differs from the exact release package"
                );
            }
        }
        let active_config = active
            .join("BepInEx")
            .join("config")
            .join("perfect-sync-smoke.cfg");
        fs::write(&active_config, b"profile-specific setting").unwrap();
        managed_instance::capture_workspace_config(&settings::profiles_root(), &profile_id)
            .unwrap();
        assert_eq!(
            fs::read(
                settings::profiles_root()
                    .join(&profile_id)
                    .join("BepInEx")
                    .join("config")
                    .join("perfect-sync-smoke.cfg")
            )
            .unwrap(),
            b"profile-specific setting"
        );
        prepare_profile_with_guard(&source, &profile_id, None, true, || Ok(())).unwrap();
        assert_eq!(
            fs::read(
                active
                    .join("BepInEx")
                    .join("config")
                    .join("perfect-sync-smoke.cfg"),
            )
            .unwrap(),
            b"profile-specific setting"
        );
        assert_eq!(fs::read(source_mira).ok(), source_mira_before);
    }

    #[test]
    #[ignore = "launches a local Epic profile through EpicGamesStarter"]
    fn live_epic_auth_launch_smoke() {
        let app_data = PathBuf::from(std::env::var("PERFECT_SYNC_SMOKE_APP_DATA").unwrap());
        let managed_data = PathBuf::from(std::env::var("PERFECT_SYNC_SMOKE_MANAGED_DATA").unwrap());
        let profile_id = std::env::var("PERFECT_SYNC_SMOKE_PROFILE").unwrap();
        settings::initialize_app_data_dir(app_data).unwrap();
        settings::initialize_managed_data_dir(managed_data).unwrap();

        let profile_record = recovered_profile_store(&settings::profiles_root())
            .unwrap()
            .load(&profile_id)
            .unwrap()
            .unwrap();
        let saved = settings::load().unwrap();
        let instance = saved
            .game_instances
            .iter()
            .find(|instance| {
                profile_record.game_instance_id.as_deref() == Some(instance.id.as_str())
            })
            .unwrap();
        assert_eq!(instance.store, Store::Epic);
        let preparation_started = Instant::now();
        prepare_profile_with_guard(&instance.path, &profile_id, None, false, || Ok(())).unwrap();
        eprintln!(
            "Epic profile preparation: {:.3}s",
            preparation_started.elapsed().as_secs_f64()
        );
        let active = managed_instance::workspace_game_dir(&profile_id).unwrap();

        let launch_started = Instant::now();
        launch_prepared_game(&active, instance, &profile_id).unwrap();

        let deadline = Instant::now() + Duration::from_secs(180);
        while Instant::now() < deadline {
            if process::try_is_game_dir_running(&active).unwrap() {
                eprintln!(
                    "Epic authentication launch: {:.3}s",
                    launch_started.elapsed().as_secs_f64()
                );
                return;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        panic!(
            "EpicGamesStarter did not launch the authenticated managed profile at {}",
            active.display()
        );
    }

    #[test]
    #[ignore = "launches the selected local Steam profile"]
    fn live_steam_launch_smoke() {
        let app_data = PathBuf::from(std::env::var("PERFECT_SYNC_SMOKE_APP_DATA").unwrap());
        let managed_data = PathBuf::from(std::env::var("PERFECT_SYNC_SMOKE_MANAGED_DATA").unwrap());
        let profile_id = std::env::var("PERFECT_SYNC_SMOKE_PROFILE").unwrap();
        settings::initialize_app_data_dir(app_data).unwrap();
        settings::initialize_managed_data_dir(managed_data).unwrap();

        let profile_record = recovered_profile_store(&settings::profiles_root())
            .unwrap()
            .load(&profile_id)
            .unwrap()
            .unwrap();
        let saved = settings::load().unwrap();
        let instance = saved
            .game_instances
            .iter()
            .find(|instance| {
                profile_record.game_instance_id.as_deref() == Some(instance.id.as_str())
            })
            .unwrap();
        assert_eq!(instance.store, Store::Steam);
        let preparation_started = Instant::now();
        prepare_profile_with_guard(&instance.path, &profile_id, None, false, || Ok(())).unwrap();
        eprintln!(
            "Steam profile preparation: {:.3}s",
            preparation_started.elapsed().as_secs_f64()
        );
        let active = managed_instance::workspace_game_dir(&profile_id).unwrap();

        let launch_started = Instant::now();
        launch_prepared_game(&active, instance, &profile_id).unwrap();

        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            if process::try_is_game_dir_running(&active).unwrap() {
                eprintln!(
                    "Steam profile launch: {:.3}s",
                    launch_started.elapsed().as_secs_f64()
                );
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        panic!(
            "Steam profile did not launch from the managed workspace at {}",
            active.display()
        );
    }
}
