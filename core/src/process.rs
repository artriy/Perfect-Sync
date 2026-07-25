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

/// Construct a background child process without allocating a console window on
/// Windows. Use `interactive_command` for helpers that require user input.
pub fn command<S: AsRef<OsStr>>(program: S) -> Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let mut command = Command::new(program);
        command.creation_flags(CREATE_NO_WINDOW);
        command
    }
    #[cfg(not(windows))]
    {
        Command::new(program)
    }
}

/// Construct an interactive child process with inherited standard streams.
/// Windows gets a dedicated visible console so GUI parents cannot hide prompts.
pub fn interactive_command<S: AsRef<OsStr>>(program: S) -> Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
        let mut command = Command::new(program);
        command.creation_flags(CREATE_NEW_CONSOLE);
        command
    }
    #[cfg(not(windows))]
    {
        Command::new(program)
    }
}

/// Launch a console helper with usable standard handles. Windows must bypass
/// `std::process::Command`: inheriting a GUI parent's empty standard handles
/// produces a visible but blank console that cannot accept input.
pub fn launch_console_interactive(
    program: &std::path::Path,
    cwd: &std::path::Path,
) -> io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;

        #[repr(C)]
        #[allow(non_snake_case)]
        struct StartupInfoW {
            cb: u32,
            lpReserved: *mut u16,
            lpDesktop: *mut u16,
            lpTitle: *mut u16,
            dwX: u32,
            dwY: u32,
            dwXSize: u32,
            dwYSize: u32,
            dwXCountChars: u32,
            dwYCountChars: u32,
            dwFillAttribute: u32,
            dwFlags: u32,
            wShowWindow: u16,
            cbReserved2: u16,
            lpReserved2: *mut u8,
            hStdInput: *mut std::ffi::c_void,
            hStdOutput: *mut std::ffi::c_void,
            hStdError: *mut std::ffi::c_void,
        }

        #[repr(C)]
        #[allow(non_snake_case)]
        struct ProcessInformation {
            hProcess: *mut std::ffi::c_void,
            hThread: *mut std::ffi::c_void,
            dwProcessId: u32,
            dwThreadId: u32,
        }

        #[link(name = "kernel32")]
        extern "system" {
            fn CreateProcessW(
                application_name: *const u16,
                command_line: *mut u16,
                process_attributes: *const std::ffi::c_void,
                thread_attributes: *const std::ffi::c_void,
                inherit_handles: i32,
                creation_flags: u32,
                environment: *const std::ffi::c_void,
                current_directory: *const u16,
                startup_info: *const StartupInfoW,
                process_information: *mut ProcessInformation,
            ) -> i32;
            fn CloseHandle(object: *mut std::ffi::c_void) -> i32;
        }

        const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
        let application_name: Vec<u16> = program.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut command_line = Vec::with_capacity(application_name.len() + 2);
        command_line.push(u16::from(b'"'));
        command_line.extend(
            application_name
                .iter()
                .copied()
                .take(application_name.len() - 1),
        );
        command_line.push(u16::from(b'"'));
        command_line.push(0);
        let current_directory: Vec<u16> = cwd.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut startup_info: StartupInfoW = unsafe { std::mem::zeroed() };
        startup_info.cb = std::mem::size_of::<StartupInfoW>() as u32;
        let mut process_information: ProcessInformation = unsafe { std::mem::zeroed() };

        // SAFETY: all strings are NUL-terminated and live for the duration of
        // the call. STARTF_USESTDHANDLES is intentionally absent, allowing the
        // new console to initialize keyboard and screen-buffer handles.
        let created = unsafe {
            CreateProcessW(
                application_name.as_ptr(),
                command_line.as_mut_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                CREATE_NEW_CONSOLE,
                std::ptr::null(),
                current_directory.as_ptr(),
                &startup_info,
                &mut process_information,
            )
        };
        if created == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: CreateProcessW initialized both handles on success. The
        // process and its console remain alive after these owner handles close.
        unsafe {
            CloseHandle(process_information.hThread);
            CloseHandle(process_information.hProcess);
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        interactive_command(program)
            .current_dir(cwd)
            .spawn()
            .map(|_| ())
    }
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

/// Spawn an interactive helper from a launch spec. Standard streams stay
/// inherited on every host; Windows also receives a dedicated console window.
pub fn launch_interactive(spec: &LaunchSpec) -> io::Result<Child> {
    if let Some(message) = &spec.error {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, message.clone()));
    }
    let mut cmd = interactive_command(&spec.program);
    cmd.current_dir(&spec.cwd);
    cmd.args(&spec.args);
    for (key, value) in &spec.env {
        cmd.env(key, value);
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
    const INTERACTIVE_CONSOLE_CHILD: &str = "PERFECT_SYNC_INTERACTIVE_CONSOLE_CHILD";

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

    #[test]
    fn interactive_helpers_get_a_console_window() {
        if std::env::var_os(INTERACTIVE_CONSOLE_CHILD).is_some() {
            // SAFETY: GetConsoleWindow has no parameters and returns the calling
            // process's console window handle, or zero when it has none.
            assert_ne!(unsafe { GetConsoleWindow() }, 0);
            return;
        }

        let output = interactive_command(std::env::current_exe().unwrap())
            .env(INTERACTIVE_CONSOLE_CHILD, "1")
            .args([
                "--exact",
                "process::tests::interactive_helpers_get_a_console_window",
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
