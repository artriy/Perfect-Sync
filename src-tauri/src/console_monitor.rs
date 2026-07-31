#[cfg(windows)]
use std::ffi::OsStr;
use std::io;
use std::path::Path;
use std::time::Duration;
#[cfg(windows)]
use std::time::Instant;

#[cfg(windows)]
pub fn wait_for_game_and_submit_enter(
    helper_pid: u32,
    game_dir: &Path,
    timeout: Duration,
) -> io::Result<()> {
    type Handle = *mut std::ffi::c_void;

    #[link(name = "kernel32")]
    extern "system" {
        fn OpenProcess(desired_access: u32, inherit_handle: i32, process_id: u32) -> Handle;
        fn WaitForSingleObject(handle: Handle, milliseconds: u32) -> u32;
        fn CloseHandle(object: Handle) -> i32;
    }

    const SYNCHRONIZE: u32 = 0x0010_0000;
    const WAIT_OBJECT_0: u32 = 0;
    const WAIT_TIMEOUT: u32 = 0x0000_0102;
    const WAIT_FAILED: u32 = u32::MAX;

    // SAFETY: OpenProcess receives a process id returned by CreateProcessW and
    // requests only wait access. The returned handle is closed on every path.
    let helper = unsafe { OpenProcess(SYNCHRONIZE, 0, helper_pid) };
    if helper.is_null() {
        return Err(io::Error::last_os_error());
    }

    let deadline = Instant::now() + timeout;
    let mut game_seen_at = None;
    let result = loop {
        match perfect_sync_core::process::try_is_game_dir_running(game_dir) {
            Ok(true) => {
                let seen_at = game_seen_at.get_or_insert_with(Instant::now);
                if seen_at.elapsed() >= Duration::from_secs(3) {
                    // Process.Start reaches the final helper prompt before this
                    // stability window elapses. Queue Enter only after this
                    // workspace's game remains alive; another profile or a
                    // crash during BepInEx startup must not satisfy launch.
                    break submit_enter_with_retry(helper_pid);
                }
            }
            Ok(false) if game_seen_at.is_some() => {
                let _ = submit_enter_with_retry(helper_pid);
                break Err(io::Error::other(
                    "The managed Among Us process exited during startup. Check BepInEx/LogOutput.log and BepInEx/ErrorLog.log for the loader failure.",
                ));
            }
            Ok(false) => {}
            Err(error) => {
                break Err(io::Error::other(format!(
                    "couldn't verify the Epic Among Us process: {error}"
                )))
            }
        }

        if Instant::now() >= deadline {
            break Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Epic authentication did not start the managed Among Us game. Complete any sign-in shown by EpicGamesStarter, then retry.",
            ));
        }

        // Waiting avoids a busy loop while still noticing helper exit.
        let wait = unsafe { WaitForSingleObject(helper, 250) };
        match wait {
            WAIT_OBJECT_0 => {
                break Err(io::Error::other(
                    "EpicGamesStarter closed before the managed Among Us game started. Retry and follow any sign-in or error shown in its console.",
                ))
            }
            WAIT_FAILED => break Err(io::Error::last_os_error()),
            WAIT_TIMEOUT => {}
            status => {
                break Err(io::Error::other(format!(
                    "unexpected process wait status {status}"
                )))
            }
        }
    };

    // SAFETY: helper is a live handle returned by OpenProcess above.
    unsafe {
        CloseHandle(helper);
    }
    result
}

#[cfg(windows)]
fn submit_enter_with_retry(helper_pid: u32) -> io::Result<()> {
    let mut last_error = None;
    for _ in 0..20 {
        match submit_enter(helper_pid) {
            Ok(()) => return Ok(()),
            Err(error) => {
                last_error = Some(error);
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
    Err(last_error.unwrap_or_else(|| io::Error::other("console input was not submitted")))
}

#[cfg(not(windows))]
pub fn wait_for_game_and_submit_enter(
    _helper_pid: u32,
    _game_dir: &Path,
    _timeout: Duration,
) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "EpicGamesStarter console monitoring is only available on Windows",
    ))
}

#[cfg(windows)]
fn submit_enter(helper_pid: u32) -> io::Result<()> {
    type Handle = *mut std::ffi::c_void;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct KeyEventRecord {
        key_down: i32,
        repeat_count: u16,
        virtual_key_code: u16,
        virtual_scan_code: u16,
        unicode_char: u16,
        control_key_state: u32,
    }

    #[repr(C)]
    union InputEvent {
        key: KeyEventRecord,
    }

    #[repr(C)]
    struct InputRecord {
        event_type: u16,
        event: InputEvent,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn AttachConsole(process_id: u32) -> i32;
        fn FreeConsole() -> i32;
        fn CreateFileW(
            file_name: *const u16,
            desired_access: u32,
            share_mode: u32,
            security_attributes: *const std::ffi::c_void,
            creation_disposition: u32,
            flags_and_attributes: u32,
            template_file: Handle,
        ) -> Handle;
        fn WriteConsoleInputW(
            console_input: Handle,
            buffer: *const InputRecord,
            length: u32,
            written: *mut u32,
        ) -> i32;
        fn CloseHandle(object: Handle) -> i32;
    }

    const KEY_EVENT: u16 = 0x0001;
    const VK_RETURN: u16 = 0x000D;
    const RETURN_SCAN_CODE: u16 = 0x001C;
    const GENERIC_READ: u32 = 0x8000_0000;
    const GENERIC_WRITE: u32 = 0x4000_0000;
    const FILE_SHARE_READ: u32 = 0x0000_0001;
    const FILE_SHARE_WRITE: u32 = 0x0000_0002;
    const OPEN_EXISTING: u32 = 3;
    const INVALID_HANDLE_VALUE: Handle = -1_isize as Handle;

    // The production monitor is launched without a console. Test harnesses and
    // unusual parent launchers can still attach one, so detach that inherited
    // console before attaching to the specific EpicGamesStarter process.
    if unsafe { AttachConsole(helper_pid) } == 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(5) {
            return Err(error);
        }
        unsafe {
            FreeConsole();
        }
        if unsafe { AttachConsole(helper_pid) } == 0 {
            return Err(io::Error::last_os_error());
        }
    }

    let result = (|| {
        let console_name: Vec<u16> = OsStr::new("CONIN$").encode_wide().chain(Some(0)).collect();
        // SAFETY: console_name is NUL-terminated and all optional pointers are null.
        let input = unsafe {
            CreateFileW(
                console_name.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null(),
                OPEN_EXISTING,
                0,
                std::ptr::null_mut(),
            )
        };
        if input == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }

        let records = [true, false].map(|key_down| InputRecord {
            event_type: KEY_EVENT,
            event: InputEvent {
                key: KeyEventRecord {
                    key_down: i32::from(key_down),
                    repeat_count: 1,
                    virtual_key_code: VK_RETURN,
                    virtual_scan_code: RETURN_SCAN_CODE,
                    unicode_char: u16::from(b'\r'),
                    control_key_state: 0,
                },
            },
        });
        let mut written = 0;
        // SAFETY: records points to two valid INPUT_RECORD-compatible values,
        // and written remains live for the duration of the call.
        let succeeded = unsafe {
            WriteConsoleInputW(input, records.as_ptr(), records.len() as u32, &mut written)
        };
        let error = if succeeded == 0 || written != records.len() as u32 {
            Some(io::Error::last_os_error())
        } else {
            None
        };
        // SAFETY: input is a valid handle returned by CreateFileW.
        unsafe {
            CloseHandle(input);
        }
        error.map_or(Ok(()), Err)
    })();

    // SAFETY: this process attached successfully above and owns that attachment.
    unsafe {
        FreeConsole();
    }
    result
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::process::Stdio;
    use std::time::{Duration, Instant};

    const CONSOLE_TARGET: &str = "PERFECT_SYNC_CONSOLE_TARGET";
    const CONSOLE_INJECTOR: &str = "PERFECT_SYNC_CONSOLE_INJECTOR";
    const CONSOLE_GAME_DIR: &str = "PERFECT_SYNC_CONSOLE_GAME_DIR";
    const READY_FILE: &str = "PERFECT_SYNC_CONSOLE_READY_FILE";
    const FAKE_GAME: &str = "PERFECT_SYNC_FAKE_GAME";
    const TRANSIENT_GAME: &str = "PERFECT_SYNC_TRANSIENT_GAME";
    const FAILURE_MONITOR: &str = "PERFECT_SYNC_FAILURE_MONITOR";

    fn read_console_enter(ready: &std::path::Path) {
        type Handle = *mut std::ffi::c_void;

        #[link(name = "kernel32")]
        extern "system" {
            fn CreateFileW(
                file_name: *const u16,
                desired_access: u32,
                share_mode: u32,
                security_attributes: *const std::ffi::c_void,
                creation_disposition: u32,
                flags_and_attributes: u32,
                template_file: Handle,
            ) -> Handle;
            fn ReadConsoleW(
                console_input: Handle,
                buffer: *mut u16,
                characters: u32,
                read: *mut u32,
                input_control: *const std::ffi::c_void,
            ) -> i32;
            fn CloseHandle(object: Handle) -> i32;
        }

        const GENERIC_READ: u32 = 0x8000_0000;
        const GENERIC_WRITE: u32 = 0x4000_0000;
        const FILE_SHARE_READ: u32 = 0x0000_0001;
        const FILE_SHARE_WRITE: u32 = 0x0000_0002;
        const OPEN_EXISTING: u32 = 3;
        const INVALID_HANDLE_VALUE: Handle = -1_isize as Handle;

        let console_name: Vec<u16> = OsStr::new("CONIN$").encode_wide().chain(Some(0)).collect();
        let input = unsafe {
            CreateFileW(
                console_name.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null(),
                OPEN_EXISTING,
                0,
                std::ptr::null_mut(),
            )
        };
        assert_ne!(input, INVALID_HANDLE_VALUE);
        fs::write(ready, b"ready").unwrap();

        let mut buffer = [0_u16; 2];
        let mut read = 0;
        let succeeded = unsafe {
            ReadConsoleW(
                input,
                buffer.as_mut_ptr(),
                buffer.len() as u32,
                &mut read,
                std::ptr::null(),
            )
        };
        unsafe {
            CloseHandle(input);
        }
        assert_ne!(succeeded, 0);
        assert!(buffer[..read as usize].contains(&u16::from(b'\r')));
    }

    #[test]
    fn only_target_game_start_submits_enter_and_releases_console_reader() {
        if std::env::var_os(FAKE_GAME).is_some() {
            std::thread::sleep(Duration::from_secs(10));
            return;
        }
        if std::env::var_os(CONSOLE_TARGET).is_some() {
            let ready = PathBuf::from(std::env::var_os(READY_FILE).unwrap());
            read_console_enter(&ready);
            return;
        }
        if let Some(pid) = std::env::var(CONSOLE_INJECTOR)
            .ok()
            .and_then(|value| value.parse().ok())
        {
            let game_dir = PathBuf::from(std::env::var_os(CONSOLE_GAME_DIR).unwrap());
            wait_for_game_and_submit_enter(pid, &game_dir, Duration::from_secs(5)).unwrap();
            return;
        }

        let temporary = tempfile::tempdir().unwrap();
        let ready = temporary.path().join("ready");
        let test_name =
            "console_monitor::tests::only_target_game_start_submits_enter_and_releases_console_reader";
        let executable = std::env::current_exe().unwrap();
        let mut target = perfect_sync_core::process::interactive_command(&executable)
            .env(CONSOLE_TARGET, "1")
            .env(READY_FILE, &ready)
            .args(["--exact", test_name])
            .spawn()
            .unwrap();

        let deadline = Instant::now() + Duration::from_secs(5);
        while !ready.is_file() && Instant::now() < deadline {
            assert!(
                target.try_wait().unwrap().is_none(),
                "console target exited before waiting"
            );
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(ready.is_file(), "console target did not become ready");

        let unrelated_dir = temporary.path().join("unrelated");
        fs::create_dir(&unrelated_dir).unwrap();
        let unrelated_path = unrelated_dir.join(perfect_sync_core::process::GAME_EXE);
        fs::copy(&executable, &unrelated_path).unwrap();
        let mut unrelated_game = perfect_sync_core::process::command(&unrelated_path)
            .env(FAKE_GAME, "1")
            .args(["--exact", test_name])
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !matches!(
            perfect_sync_core::process::try_is_game_dir_running(&unrelated_dir),
            Ok(true)
        ) && Instant::now() < deadline
        {
            assert!(
                unrelated_game.try_wait().unwrap().is_none(),
                "unrelated game exited before detection"
            );
            std::thread::sleep(Duration::from_millis(25));
        }

        let mut injector = perfect_sync_core::process::command(&executable)
            .env(CONSOLE_INJECTOR, target.id().to_string())
            .env(CONSOLE_GAME_DIR, temporary.path())
            .args(["--exact", test_name])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        std::thread::sleep(Duration::from_millis(500));
        assert!(
            injector.try_wait().unwrap().is_none(),
            "an unrelated Among Us process incorrectly completed the monitor"
        );
        assert!(
            target.try_wait().unwrap().is_none(),
            "an unrelated Among Us process incorrectly submitted Enter"
        );

        let fake_game = temporary.path().join(perfect_sync_core::process::GAME_EXE);
        fs::copy(&executable, &fake_game).unwrap();
        let mut game = perfect_sync_core::process::command(&fake_game)
            .env(FAKE_GAME, "1")
            .args(["--exact", test_name])
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !matches!(
            perfect_sync_core::process::try_is_game_dir_running(temporary.path()),
            Ok(true)
        ) && Instant::now() < deadline
        {
            assert!(
                game.try_wait().unwrap().is_none(),
                "target game exited before detection"
            );
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(
            matches!(
                perfect_sync_core::process::try_is_game_dir_running(temporary.path()),
                Ok(true)
            ),
            "target game was not detected in its exact directory"
        );

        let injector_output = injector.wait_with_output().unwrap();
        let _ = game.kill();
        let _ = game.wait();
        let _ = unrelated_game.kill();
        let _ = unrelated_game.wait();
        assert!(
            injector_output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&injector_output.stdout),
            String::from_utf8_lossy(&injector_output.stderr)
        );

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = target.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            if Instant::now() >= deadline {
                target.kill().unwrap();
                panic!("console target did not consume injected Enter");
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }
    #[test]
    fn transient_target_game_is_a_launch_failure() {
        if std::env::var_os(TRANSIENT_GAME).is_some() {
            std::thread::sleep(Duration::from_secs(1));
            return;
        }
        if std::env::var_os(CONSOLE_TARGET).is_some() {
            let ready = PathBuf::from(std::env::var_os(READY_FILE).unwrap());
            read_console_enter(&ready);
            return;
        }
        if let Some(pid) = std::env::var(FAILURE_MONITOR)
            .ok()
            .and_then(|value| value.parse().ok())
        {
            let game_dir = PathBuf::from(std::env::var_os(CONSOLE_GAME_DIR).unwrap());
            let error =
                wait_for_game_and_submit_enter(pid, &game_dir, Duration::from_secs(5)).unwrap_err();
            assert!(error.to_string().contains("exited during startup"));
            return;
        }

        let temporary = tempfile::tempdir().unwrap();
        let ready = temporary.path().join("ready");
        let test_name = "console_monitor::tests::transient_target_game_is_a_launch_failure";
        let executable = std::env::current_exe().unwrap();
        let mut target = perfect_sync_core::process::interactive_command(&executable)
            .env(CONSOLE_TARGET, "1")
            .env(READY_FILE, &ready)
            .args(["--exact", test_name])
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !ready.is_file() && Instant::now() < deadline {
            assert!(target.try_wait().unwrap().is_none());
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(ready.is_file(), "console target did not become ready");

        let monitor = perfect_sync_core::process::command(&executable)
            .env(FAILURE_MONITOR, target.id().to_string())
            .env(CONSOLE_GAME_DIR, temporary.path())
            .args(["--exact", test_name])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let fake_game = temporary.path().join(perfect_sync_core::process::GAME_EXE);
        fs::copy(&executable, &fake_game).unwrap();
        let mut game = perfect_sync_core::process::command(&fake_game)
            .env(TRANSIENT_GAME, "1")
            .args(["--exact", test_name])
            .spawn()
            .unwrap();

        let monitor_output = monitor.wait_with_output().unwrap();
        let _ = game.wait();
        assert!(
            monitor_output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&monitor_output.stdout),
            String::from_utf8_lossy(&monitor_output.stderr)
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = target.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            if Instant::now() >= deadline {
                target.kill().unwrap();
                panic!("console target did not consume injected Enter after the game crash");
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }
}

#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
