//! GameProcess: build the launch invocation (pure, testable) and provide thin
//! OS-bound helpers to detect whether Among Us is running and to spawn it.
//!
//! All file mutations must be gated on the game NOT running (file locks), so
//! callers check `is_running()` before installing/launching.

use crate::loader;
use std::path::{Path, PathBuf};

pub const GAME_EXE: &str = "Among Us.exe";

/// A fully-resolved launch: the executable to run plus the Doorstop environment
/// that redirects it at a specific profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchSpec {
    pub program: PathBuf,
    pub env: Vec<(String, String)>,
}

/// Build the launch spec for `game_dir` against `profile_dir` (pure).
pub fn build_launch(game_dir: &Path, profile_dir: &Path) -> LaunchSpec {
    LaunchSpec {
        program: game_dir.join(GAME_EXE),
        env: loader::launch_env(profile_dir),
    }
}

/// Whether an Among Us process is currently running (Windows: via `tasklist`).
pub fn is_running() -> bool {
    #[cfg(windows)]
    {
        if let Ok(out) = std::process::Command::new("tasklist")
            .args(["/FI", &format!("IMAGENAME eq {GAME_EXE}"), "/NH"])
            .output()
        {
            return String::from_utf8_lossy(&out.stdout).contains(GAME_EXE);
        }
        false
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Spawn the game with the launch spec's environment. Caller must ensure the
/// game is not already running.
pub fn launch(spec: &LaunchSpec) -> std::io::Result<std::process::Child> {
    let mut cmd = std::process::Command::new(&spec.program);
    if let Some(dir) = spec.program.parent() {
        cmd.current_dir(dir);
    }
    for (k, v) in &spec.env {
        cmd.env(k, v);
    }
    cmd.spawn()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_launch_targets_exe_and_profile() {
        let spec = build_launch(Path::new("/games/Among Us"), Path::new("/profiles/p1"));
        assert!(spec.program.ends_with("Among Us.exe"));
        assert!(spec.program.starts_with("/games/Among Us"));
        assert!(spec
            .env
            .iter()
            .any(|(k, v)| k == "DOORSTOP_TARGET_ASSEMBLY" && v.contains("p1")));
        assert!(spec.env.iter().any(|(k, _)| k == "DOORSTOP_ENABLED"));
    }
}
