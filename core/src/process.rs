//! GameProcess: detect whether Among Us is running and spawn a built launch
//! spec. The per-runtime launch invocation is built in `compat` (Among Us is a
//! Windows build, so off Windows it runs under Proton/Wine/CrossOver); this
//! module stays a thin OS-bound layer.
//!
//! All file mutations must be gated on the game NOT running (file locks), so
//! callers check `is_running()` before installing/launching.

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
#[cfg(any(not(windows), test))]
use std::time::{Duration, Instant};

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

#[cfg(windows)]
fn win32_conventional_path(path: &std::path::Path) -> PathBuf {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};

    let wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    let verbatim = [
        u16::from(b'\\'),
        u16::from(b'\\'),
        u16::from(b'?'),
        u16::from(b'\\'),
    ];
    if !wide.starts_with(&verbatim) {
        return path.to_path_buf();
    }
    let unc = wide.len() >= 8
        && matches!(wide[4], value if value == u16::from(b'U') || value == u16::from(b'u'))
        && matches!(wide[5], value if value == u16::from(b'N') || value == u16::from(b'n'))
        && matches!(wide[6], value if value == u16::from(b'C') || value == u16::from(b'c'))
        && wide[7] == u16::from(b'\\');
    let conventional = if unc {
        let mut conventional = vec![u16::from(b'\\'), u16::from(b'\\')];
        conventional.extend_from_slice(&wide[8..]);
        conventional
    } else {
        wide[verbatim.len()..].to_vec()
    };
    PathBuf::from(OsString::from_wide(&conventional))
}

#[cfg(windows)]
fn win32_current_directory(path: &std::path::Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;

    win32_conventional_path(path)
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect()
}

/// Launch a console helper with usable standard handles. Windows must bypass
/// `std::process::Command`: inheriting a GUI parent's empty standard handles
/// produces a visible but blank console that cannot accept input.
pub fn launch_console_interactive(
    program: &std::path::Path,
    cwd: &std::path::Path,
) -> io::Result<u32> {
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
        let program = win32_conventional_path(program);
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
        // BepInEx and .NET preserve a verbatim prefix from either the helper
        // image or its current directory. Older Cpp2IL builds then reject valid
        // game files below that path, so both process paths cross the Win32
        // boundary in equivalent conventional syntax.
        let current_directory = win32_current_directory(cwd);
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
        // caller only needs the process ID; the process and console remain
        // alive after these owner handles close.
        unsafe {
            CloseHandle(process_information.hThread);
            CloseHandle(process_information.hProcess);
        }
        Ok(process_information.dwProcessId)
    }
    #[cfg(not(windows))]
    {
        interactive_command(program)
            .current_dir(cwd)
            .spawn()
            .map(|child| child.id())
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

#[cfg(windows)]
fn windows_path_key(path: &Path) -> String {
    let mut value = path.to_string_lossy().replace('/', "\\");
    if value.len() >= 8 && value[..8].eq_ignore_ascii_case(r"\\?\UNC\") {
        value = format!(r"\\{}", &value[8..]);
    } else if value.len() >= 4 && value[..4].eq_ignore_ascii_case(r"\\?\") {
        value = value[4..].to_string();
    }
    value.trim_end_matches('\\').to_ascii_lowercase()
}

#[cfg(windows)]
struct RunningGameProcess {
    pid: u32,
    path: PathBuf,
}

#[cfg(windows)]
fn running_game_processes() -> io::Result<Vec<RunningGameProcess>> {
    use std::os::windows::ffi::OsStringExt;

    type Handle = *mut std::ffi::c_void;

    #[repr(C)]
    #[allow(non_snake_case)]
    struct ProcessEntry32W {
        dwSize: u32,
        cntUsage: u32,
        th32ProcessID: u32,
        th32DefaultHeapID: usize,
        th32ModuleID: u32,
        cntThreads: u32,
        th32ParentProcessID: u32,
        pcPriClassBase: i32,
        dwFlags: u32,
        szExeFile: [u16; 260],
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateToolhelp32Snapshot(flags: u32, process_id: u32) -> Handle;
        fn Process32FirstW(snapshot: Handle, entry: *mut ProcessEntry32W) -> i32;
        fn Process32NextW(snapshot: Handle, entry: *mut ProcessEntry32W) -> i32;
        fn OpenProcess(access: u32, inherit_handle: i32, process_id: u32) -> Handle;
        fn QueryFullProcessImageNameW(
            process: Handle,
            flags: u32,
            executable_name: *mut u16,
            size: *mut u32,
        ) -> i32;
        fn CloseHandle(object: Handle) -> i32;
    }

    const TH32CS_SNAPPROCESS: u32 = 0x0000_0002;
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x0000_1000;
    const ERROR_NO_MORE_FILES: i32 = 18;
    let invalid_handle = -1_isize as Handle;
    // SAFETY: the snapshot call has no borrowed pointer arguments.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == invalid_handle {
        return Err(io::Error::last_os_error());
    }
    let mut entry: ProcessEntry32W = unsafe { std::mem::zeroed() };
    entry.dwSize = std::mem::size_of::<ProcessEntry32W>() as u32;
    // SAFETY: `entry` has the documented size and remains live for enumeration.
    let mut present = unsafe { Process32FirstW(snapshot, &mut entry) } != 0;
    if !present {
        let error = io::Error::last_os_error();
        // SAFETY: `snapshot` is a live handle returned above.
        unsafe { CloseHandle(snapshot) };
        return if error.raw_os_error() == Some(ERROR_NO_MORE_FILES) {
            Ok(Vec::new())
        } else {
            Err(error)
        };
    }

    let mut paths = Vec::new();
    while present {
        let name_end = entry
            .szExeFile
            .iter()
            .position(|character| *character == 0)
            .unwrap_or(entry.szExeFile.len());
        let executable_name = OsString::from_wide(&entry.szExeFile[..name_end]);
        if executable_name
            .to_string_lossy()
            .eq_ignore_ascii_case(GAME_EXE)
        {
            // Access-denied races are normal for processes exiting during the snapshot.
            // SAFETY: the process ID came from the live Toolhelp snapshot.
            let process =
                unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, entry.th32ProcessID) };
            if !process.is_null() {
                let mut buffer = vec![0_u16; 32_768];
                let mut size = buffer.len() as u32;
                // SAFETY: the writable buffer has `size` UTF-16 elements.
                let queried = unsafe {
                    QueryFullProcessImageNameW(process, 0, buffer.as_mut_ptr(), &mut size)
                };
                if queried != 0 {
                    paths.push(RunningGameProcess {
                        pid: entry.th32ProcessID,
                        path: PathBuf::from(OsString::from_wide(&buffer[..size as usize])),
                    });
                }
                // SAFETY: `process` is a live handle returned by OpenProcess.
                unsafe { CloseHandle(process) };
            }
        }
        // SAFETY: `entry` remains initialized with its documented size.
        present = unsafe { Process32NextW(snapshot, &mut entry) } != 0;
    }
    // SAFETY: `snapshot` is a live handle returned above.
    unsafe { CloseHandle(snapshot) };
    Ok(paths)
}

/// Query whether the Among Us executable inside one concrete game directory is
/// running. Unlike the global query, another profile or source does not match.
pub fn try_is_game_dir_running(game_dir: &Path) -> io::Result<bool> {
    let executable = game_dir.join(GAME_EXE);
    #[cfg(windows)]
    {
        let expected = windows_path_key(&executable);
        Ok(running_game_processes()?
            .iter()
            .any(|process| windows_path_key(&process.path) == expected))
    }
    #[cfg(not(windows))]
    {
        Ok(!unix_game_pids(&unix_process_output()?.stdout, &executable, false).is_empty())
    }
}

#[cfg(any(not(windows), test))]
fn is_crossover_dispatcher(command_line: &str) -> bool {
    let normalized = command_line.replace('\\', "/").to_ascii_lowercase();
    normalized.contains("/contents/sharedsupport/crossover/")
        && normalized.contains("/wine --bottle")
        && !normalized.contains("/wineloader")
}

#[cfg(any(not(windows), test))]
fn unix_game_pids(output: &[u8], executable: &Path, include_dispatchers: bool) -> Vec<u32> {
    let expected = executable.to_string_lossy();
    let wine_path = expected
        .strip_prefix('/')
        .map(|path| format!(r"Z:\{}", path.replace('/', "\\")));
    String::from_utf8_lossy(output)
        .lines()
        .filter_map(|line| {
            let line = line.trim_start();
            let (pid, command_line) = line.split_once(char::is_whitespace)?;
            let command_line = command_line.trim_start();
            let executable_matches = command_line.contains(expected.as_ref())
                || wine_path
                    .as_deref()
                    .is_some_and(|path| command_line.contains(path));
            if !executable_matches
                || (!include_dispatchers && is_crossover_dispatcher(command_line))
            {
                return None;
            }
            pid.parse().ok()
        })
        .collect()
}

#[cfg(not(windows))]
fn unix_process_output() -> io::Result<Output> {
    let output = command("ps")
        .args(["-axww", "-o", "pid=,command="])
        .output()?;
    if output.status.success() {
        Ok(output)
    } else {
        Err(query_failed("ps", &output))
    }
}

pub fn terminate_game_dir(game_dir: &Path) -> io::Result<bool> {
    let executable = game_dir.join(GAME_EXE);
    #[cfg(windows)]
    {
        let expected = windows_path_key(&executable);
        let processes = running_game_processes()?
            .into_iter()
            .filter(|process| windows_path_key(&process.path) == expected)
            .collect::<Vec<_>>();
        if processes.is_empty() {
            return Ok(false);
        }
        let mut termination_error = None;
        for process in processes {
            let output = command("taskkill")
                .args(["/PID", &process.pid.to_string(), "/T", "/F"])
                .output()?;
            if !output.status.success() {
                termination_error = Some(query_failed("taskkill", &output));
            }
        }
        if try_is_game_dir_running(game_dir)? {
            return Err(termination_error
                .unwrap_or_else(|| io::Error::other("Among Us did not stop after taskkill")));
        }
        Ok(true)
    }
    #[cfg(not(windows))]
    {
        let processes = unix_game_pids(&unix_process_output()?.stdout, &executable, true);
        if processes.is_empty() {
            return Ok(false);
        }
        let mut termination_error = None;
        for pid in processes {
            let output = command("kill").args(["-TERM", &pid.to_string()]).output()?;
            if !output.status.success() {
                termination_error = Some(query_failed("kill", &output));
            }
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut remaining;
        loop {
            remaining = unix_game_pids(&unix_process_output()?.stdout, &executable, true);
            if remaining.is_empty() {
                return Ok(true);
            }
            if Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        for pid in remaining {
            let output = command("kill").args(["-KILL", &pid.to_string()]).output()?;
            if !output.status.success() {
                termination_error = Some(query_failed("kill", &output));
            }
        }
        if !unix_game_pids(&unix_process_output()?.stdout, &executable, true).is_empty() {
            return Err(termination_error
                .unwrap_or_else(|| io::Error::other("Among Us did not stop after SIGKILL")));
        }
        Ok(true)
    }
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
    #[cfg(windows)]
    let mut cmd = command(win32_conventional_path(&spec.program));
    #[cfg(not(windows))]
    let mut cmd = command(&spec.program);
    #[cfg(windows)]
    cmd.current_dir(win32_conventional_path(&spec.cwd));
    #[cfg(not(windows))]
    cmd.current_dir(&spec.cwd);
    // The Tauri Windows executable has no console and can retain stale standard
    // handle values. Never ask CreateProcess to inherit those handles for a
    // background game; doing so fails before startup with ERROR_INVALID_HANDLE.
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
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
    #[cfg(windows)]
    let mut cmd = interactive_command(win32_conventional_path(&spec.program));
    #[cfg(not(windows))]
    let mut cmd = interactive_command(&spec.program);
    #[cfg(windows)]
    cmd.current_dir(win32_conventional_path(&spec.cwd));
    #[cfg(not(windows))]
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

    #[test]
    fn unix_process_matching_ignores_crossover_dispatch_wrapper() {
        let game = Path::new("/Users/u/Perfect Sync/Main/current/Among Us.exe");
        let output = concat!(
            "  101 /usr/bin/perl /Applications/CrossOver.app/Contents/SharedSupport/CrossOver/bin/wine --bottle AU -- /Users/u/Perfect Sync/Main/current/Among Us.exe\n",
            "  102 /Applications/CrossOver.app/Contents/SharedSupport/CrossOver/lib/wine/x86_64-unix/wineloader Z:\\Users\\u\\Perfect Sync\\Main\\current\\Among Us.exe\n",
            "  103 /usr/bin/perl /Applications/CrossOver.app/Contents/SharedSupport/CrossOver/bin/wine --bottle AU -- /Users/u/Perfect Sync/Other/current/Among Us.exe\n",
            "  104 /usr/bin/wine64 /Users/u/Perfect Sync/Main/current/Among Us.exe\n",
        );

        assert_eq!(
            unix_game_pids(output.as_bytes(), game, false),
            vec![102, 104]
        );
        assert_eq!(
            unix_game_pids(output.as_bytes(), game, true),
            vec![101, 102, 104]
        );
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn terminate_game_dir_stops_only_the_matching_game_executable() {
        const CHILD: &str = "PERFECT_SYNC_TERMINATE_GAME_CHILD";
        const TEST: &str =
            "process::tests::terminate_game_dir_stops_only_the_matching_game_executable";

        if std::env::var_os(CHILD).is_some() {
            std::thread::sleep(Duration::from_secs(30));
            return;
        }

        let game_dir = tempfile::tempdir().unwrap();
        let game_executable = game_dir.path().join(GAME_EXE);
        std::fs::copy(std::env::current_exe().unwrap(), &game_executable).unwrap();
        let mut game = Command::new(&game_executable)
            .env(CHILD, "1")
            .args(["--exact", TEST, "--nocapture"])
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !try_is_game_dir_running(game_dir.path()).unwrap() {
            assert!(Instant::now() < deadline, "game process was not detected");
            std::thread::sleep(Duration::from_millis(50));
        }

        assert!(terminate_game_dir(game_dir.path()).unwrap());
        assert!(!try_is_game_dir_running(game_dir.path()).unwrap());
        game.wait().unwrap();
    }

    const NO_CONSOLE_CHILD: &str = "PERFECT_SYNC_NO_CONSOLE_CHILD";
    const INTERACTIVE_CONSOLE_CHILD: &str = "PERFECT_SYNC_INTERACTIVE_CONSOLE_CHILD";
    const PATH_QUERY_CHILD: &str = "PERFECT_SYNC_PATH_QUERY_CHILD";
    const INVALID_STDIO_PARENT: &str = "PERFECT_SYNC_INVALID_STDIO_PARENT";
    const LAUNCH_TARGET: &str = "PERFECT_SYNC_LAUNCH_TARGET";

    #[link(name = "Kernel32")]
    extern "system" {
        fn GetConsoleWindow() -> isize;
        fn SetStdHandle(standard_handle: u32, handle: isize) -> i32;
    }

    #[test]
    fn game_launch_survives_parent_with_invalid_standard_handles() {
        const TEST_NAME: &str =
            "process::tests::game_launch_survives_parent_with_invalid_standard_handles";
        if std::env::var_os(LAUNCH_TARGET).is_some() {
            return;
        }
        if std::env::var_os(INVALID_STDIO_PARENT).is_some() {
            const STD_INPUT_HANDLE: u32 = -10_i32 as u32;
            const STD_OUTPUT_HANDLE: u32 = -11_i32 as u32;
            const STD_ERROR_HANDLE: u32 = -12_i32 as u32;
            const CLOSED_HANDLE_VALUE: isize = 0x1234;
            // SAFETY: this branch runs in an isolated subprocess created below.
            // Replacing its process-wide standard handles cannot affect the test
            // harness or another test process. The non-null values model stale
            // GUI-parent handles which cannot be inherited by a new child.
            unsafe {
                assert_ne!(SetStdHandle(STD_INPUT_HANDLE, CLOSED_HANDLE_VALUE), 0);
                assert_ne!(SetStdHandle(STD_OUTPUT_HANDLE, CLOSED_HANDLE_VALUE), 0);
                assert_ne!(SetStdHandle(STD_ERROR_HANDLE, CLOSED_HANDLE_VALUE), 0);
            }
            let executable = std::env::current_exe().unwrap();
            let mut child = launch(&LaunchSpec {
                program: executable,
                args: vec![OsString::from("--exact"), OsString::from(TEST_NAME)],
                cwd: std::env::current_dir().unwrap(),
                env: vec![(OsString::from(LAUNCH_TARGET), OsString::from("1"))],
                error: None,
            })
            .unwrap();
            assert!(child.wait().unwrap().success());
            return;
        }

        let output = Command::new(std::env::current_exe().unwrap())
            .env(INVALID_STDIO_PARENT, "1")
            .args(["--exact", TEST_NAME])
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
    fn process_paths_remove_verbatim_syntax() {
        use std::os::windows::ffi::OsStringExt;

        let decoded = |path| {
            let mut wide = win32_current_directory(std::path::Path::new(path));
            assert_eq!(wide.pop(), Some(0));
            OsString::from_wide(&wide)
        };
        assert_eq!(
            win32_conventional_path(std::path::Path::new(
                r"\\?\D:\Epic Games Games\AmongUs - TOU\EpicGamesStarter.exe"
            )),
            PathBuf::from(r"D:\Epic Games Games\AmongUs - TOU\EpicGamesStarter.exe")
        );
        assert_eq!(
            decoded(r"\\?\D:\Epic Games Games\AmongUs - TOU"),
            OsString::from(r"D:\Epic Games Games\AmongUs - TOU")
        );
        assert_eq!(
            decoded(r"\\?\UNC\server\share\Among Us"),
            OsString::from(r"\\server\share\Among Us")
        );
        assert_eq!(
            decoded(r"D:\SteamLibrary\Among Us"),
            OsString::from(r"D:\SteamLibrary\Among Us")
        );
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
    #[test]
    fn path_specific_query_tracks_concurrent_game_directories_independently() {
        if std::env::var_os(PATH_QUERY_CHILD).is_some() {
            std::thread::sleep(std::time::Duration::from_secs(5));
            return;
        }

        let temp = tempfile::tempdir().unwrap();
        let first_dir = temp.path().join("steam-profile");
        let second_dir = temp.path().join("epic-profile");
        std::fs::create_dir(&first_dir).unwrap();
        std::fs::create_dir(&second_dir).unwrap();
        let current_executable = std::env::current_exe().unwrap();
        let first_executable = first_dir.join(GAME_EXE);
        let second_executable = second_dir.join(GAME_EXE);
        std::fs::copy(&current_executable, &first_executable).unwrap();
        std::fs::copy(&current_executable, &second_executable).unwrap();
        let spawn_game = |executable: &Path| {
            command(executable)
                .env(PATH_QUERY_CHILD, "1")
                .args([
                    "--exact",
                    "process::tests::path_specific_query_tracks_concurrent_game_directories_independently",
                ])
                .spawn()
                .unwrap()
        };
        let mut first = spawn_game(&first_executable);
        let mut second = spawn_game(&second_executable);

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            if try_is_game_dir_running(&first_dir).unwrap()
                && try_is_game_dir_running(&second_dir).unwrap()
            {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "concurrent copied Among Us.exe processes were not detected"
            );
            std::thread::sleep(std::time::Duration::from_millis(25));
        }

        first.kill().unwrap();
        first.wait().unwrap();
        assert!(!try_is_game_dir_running(&first_dir).unwrap());
        assert!(try_is_game_dir_running(&second_dir).unwrap());
        second.kill().unwrap();
        second.wait().unwrap();
    }
}
