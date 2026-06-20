//! GameProcess: detect whether Among Us is running and spawn a built launch
//! spec. The per-runtime launch invocation is built in `compat` (Among Us is a
//! Windows build, so off Windows it runs under Proton/Wine/CrossOver); this
//! module stays a thin OS-bound layer.
//!
//! All file mutations must be gated on the game NOT running (file locks), so
//! callers check `is_running()` before installing/launching.

use std::path::PathBuf;
use std::process::{Child, Command};

pub const GAME_EXE: &str = "Among Us.exe";

/// A fully-resolved launch: program + args + working dir + environment. On
/// Windows `program` is the game exe; under Wine/Proton it is the wine/steam
/// launcher with the exe (or app id) passed in `args`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchSpec {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: Vec<(String, String)>,
}

/// Whether an Among Us process is currently running. Windows uses `tasklist`;
/// elsewhere `pgrep` (Wine names the process after the exe, so `-f Among Us.exe`
/// matches the game running under Proton/Wine/CrossOver).
pub fn is_running() -> bool {
    if cfg!(windows) {
        Command::new("tasklist")
            .args(["/FI", &format!("IMAGENAME eq {GAME_EXE}"), "/NH"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains(GAME_EXE))
            .unwrap_or(false)
    } else {
        Command::new("pgrep")
            .args(["-f", GAME_EXE])
            .output()
            .map(|o| o.status.success() && !o.stdout.is_empty())
            .unwrap_or(false)
    }
}

/// Spawn the game from a launch spec. Caller must ensure it is not already running.
pub fn launch(spec: &LaunchSpec) -> std::io::Result<Child> {
    let mut cmd = Command::new(&spec.program);
    cmd.current_dir(&spec.cwd);
    cmd.args(&spec.args);
    for (k, v) in &spec.env {
        cmd.env(k, v);
    }
    cmd.spawn()
}
