//! GameProcess: detect whether Among Us is running and spawn a built launch
//! spec. The per-runtime launch invocation is built in `compat` (Among Us is a
//! Windows build, so off Windows it runs under Proton/Wine/CrossOver); this
//! module stays a thin OS-bound layer.
//!
//! All file mutations must be gated on the game NOT running (file locks), so
//! callers check `is_running()` before installing/launching.

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::PathBuf;
use std::process::{Child, Command, Output};

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

/// A fully-resolved, structured launch. Arguments and environment entries stay
/// as native OS strings so paths are never lossy or interpreted by a shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchSpec {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub cwd: PathBuf,
    pub env: Vec<(OsString, OsString)>,
    /// A resolution failure which must be returned before any process is spawned.
    pub error: Option<String>,
}

/// Query whether Among Us is running. A helper failure is distinct from a
/// successful query which found no matching process.
pub fn try_is_running() -> io::Result<bool> {
    if cfg!(windows) {
        interpret_tasklist(
            command("tasklist")
                .args(["/FI", &format!("IMAGENAME eq {GAME_EXE}"), "/NH"])
                .output(),
        )
    } else {
        interpret_pgrep(command("pgrep").args(["-f", GAME_EXE]).output())
    }
}

fn query_failed(helper: &str, output: &Output) -> io::Error {
    io::Error::other(format!(
        "{helper} process query failed with status {}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr).trim()
    ))
}

fn interpret_tasklist(result: io::Result<Output>) -> io::Result<bool> {
    let output = result?;
    if !output.status.success() {
        return Err(query_failed("tasklist", &output));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .to_ascii_lowercase()
        .contains(&GAME_EXE.to_ascii_lowercase()))
}

fn pgrep_state(status_code: Option<i32>, has_output: bool) -> Option<bool> {
    match status_code {
        Some(0) => Some(has_output),
        Some(1) => Some(false),
        _ => None,
    }
}

fn interpret_pgrep(result: io::Result<Output>) -> io::Result<bool> {
    let output = result?;
    pgrep_state(output.status.code(), !output.stdout.is_empty())
        .ok_or_else(|| query_failed("pgrep", &output))
}

/// Compatibility wrapper for existing boolean callsites. Query failures are
/// conservatively treated as running so mutations remain fail closed.
pub fn is_running() -> bool {
    try_is_running().unwrap_or(true)
}

/// Spawn the game from a launch spec. Caller must ensure it is not already running.
pub fn launch(spec: &LaunchSpec) -> io::Result<Child> {
    if let Some(message) = &spec.error {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, message.clone()));
    }
    let mut cmd = command(&spec.program);
    cmd.current_dir(&spec.cwd);
    cmd.args(&spec.args);
    for (k, v) in &spec.env {
        cmd.env(k, v);
    }
    cmd.spawn()
}

#[cfg(test)]
mod query_tests {
    use super::*;

    #[test]
    fn pgrep_distinguishes_absence_from_query_failure() {
        assert_eq!(pgrep_state(Some(1), false), Some(false));
        assert_eq!(pgrep_state(Some(0), true), Some(true));
        assert_eq!(pgrep_state(Some(2), false), None);
        assert_eq!(pgrep_state(None, false), None);
    }
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
