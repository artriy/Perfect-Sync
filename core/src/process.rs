//! GameProcess: detect whether Among Us is running and spawn a built launch
//! spec. The per-runtime launch invocation is built in `compat` (Among Us is a
//! Windows build, so off Windows it runs under Proton/Wine/CrossOver); this
//! module stays a thin OS-bound layer.
//!
//! All file mutations must be gated on the game NOT running (file locks), so
//! callers check `is_running()` before installing/launching.

use std::ffi::OsStr;
use std::path::PathBuf;
use std::process::{Child, Command};

pub const GAME_EXE: &str = "Among Us.exe";

/// Construct a child process without ever allocating a console window on
/// Windows. Every process spawned by the application goes through this helper.
pub fn command<S: AsRef<OsStr>>(program: S) -> Command {
    let mut command = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

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
        command("tasklist")
            .args(["/FI", &format!("IMAGENAME eq {GAME_EXE}"), "/NH"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains(GAME_EXE))
            .unwrap_or(false)
    } else {
        command("pgrep")
            .args(["-f", GAME_EXE])
            .output()
            .map(|o| o.status.success() && !o.stdout.is_empty())
            .unwrap_or(false)
    }
}

/// Spawn the game from a launch spec. Caller must ensure it is not already running.
pub fn launch(spec: &LaunchSpec) -> std::io::Result<Child> {
    let mut cmd = command(&spec.program);
    cmd.current_dir(&spec.cwd);
    cmd.args(&spec.args);
    for (k, v) in &spec.env {
        cmd.env(k, v);
    }
    cmd.spawn()
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    const NO_CONSOLE_CHILD: &str = "PERFECT_SYNC_NO_CONSOLE_CHILD";

    #[link(name = "Kernel32")]
    extern "system" {
        fn GetConsoleWindow() -> isize;
    }

    #[test]
    fn spawned_helpers_have_no_console_window() {
        if std::env::var_os(NO_CONSOLE_CHILD).is_some() {
            // SAFETY: GetConsoleWindow has no parameters and returns the calling
            // process's console window handle, or zero when it has none.
            assert_eq!(unsafe { GetConsoleWindow() }, 0);
            return;
        }

        let output = command(std::env::current_exe().unwrap())
            .env(NO_CONSOLE_CHILD, "1")
            .args([
                "--exact",
                "process::tests::spawned_helpers_have_no_console_window",
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
