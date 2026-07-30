use std::ffi::OsStr;
use std::io;

const MONITOR_ARG: &str = "--perfect-sync-monitor-epic-console";

pub fn run_if_requested() -> bool {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some(OsStr::new(MONITOR_ARG)) {
        return false;
    }

    #[cfg(windows)]
    if let Some(pid) = args
        .next()
        .and_then(|value| value.to_str().and_then(|value| value.parse::<u32>().ok()))
        .filter(|pid| *pid != 0)
    {
        let _ = wait_for_game_and_submit_enter(pid);
    }
    true
}

#[cfg(windows)]
pub fn start(helper_pid: u32) -> io::Result<()> {
    #[cfg(test)]
    {
        std::thread::Builder::new()
            .name("epic-console-monitor".into())
            .spawn(move || {
                let _ = wait_for_game_and_submit_enter(helper_pid);
            })
            .map(|_| ())
    }
    #[cfg(not(test))]
    {
        let executable = std::env::current_exe()?;
        perfect_sync_core::process::command(executable)
            .arg(MONITOR_ARG)
            .arg(helper_pid.to_string())
            .spawn()
            .map(|_| ())
    }
}

#[cfg(not(windows))]
pub fn start(_helper_pid: u32) -> io::Result<()> {
    Ok(())
}

#[cfg(windows)]
fn wait_for_game_and_submit_enter(helper_pid: u32) -> io::Result<()> {
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

    let result = loop {
        // Waiting first avoids a busy loop while still noticing helper exit.
        let wait = unsafe { WaitForSingleObject(helper, 250) };
        match wait {
            WAIT_OBJECT_0 => break Ok(()),
            WAIT_FAILED => break Err(io::Error::last_os_error()),
            WAIT_TIMEOUT => {}
            status => {
                break Err(io::Error::other(format!(
                    "unexpected process wait status {status}"
                )))
            }
        }

        if matches!(perfect_sync_core::process::try_is_running(), Ok(true)) {
            // Process.Start returns just before EpicGamesStarter reaches
            // Console.ReadKey. A queued Enter is safe and will be consumed as
            // soon as that final success prompt begins waiting.
            let mut last_error = None;
            let mut submitted = false;
            for _ in 0..20 {
                match submit_enter(helper_pid) {
                    Ok(()) => {
                        submitted = true;
                        break;
                    }
                    Err(error) => {
                        last_error = Some(error);
                        std::thread::sleep(std::time::Duration::from_millis(50));
                    }
                }
            }
            break if submitted {
                Ok(())
            } else {
                Err(last_error
                    .unwrap_or_else(|| io::Error::other("console input was not submitted")))
            };
        }
    };

    // SAFETY: helper is a live handle returned by OpenProcess above.
    unsafe {
        CloseHandle(helper);
    }
    result
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
    use std::time::{Duration, Instant};

    const CONSOLE_TARGET: &str = "PERFECT_SYNC_CONSOLE_TARGET";
    const CONSOLE_INJECTOR: &str = "PERFECT_SYNC_CONSOLE_INJECTOR";
    const READY_FILE: &str = "PERFECT_SYNC_CONSOLE_READY_FILE";
    const FAKE_GAME: &str = "PERFECT_SYNC_FAKE_GAME";

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
    fn game_start_submits_enter_and_releases_console_reader() {
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
            wait_for_game_and_submit_enter(pid).unwrap();
            return;
        }

        let temporary = tempfile::tempdir().unwrap();
        let ready = temporary.path().join("ready");
        let test_name =
            "console_monitor::tests::game_start_submits_enter_and_releases_console_reader";
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
        let fake_game = temporary.path().join(perfect_sync_core::process::GAME_EXE);
        fs::copy(&executable, &fake_game).unwrap();
        let mut game = perfect_sync_core::process::command(&fake_game)
            .env(FAKE_GAME, "1")
            .args(["--exact", test_name])
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !matches!(perfect_sync_core::process::try_is_running(), Ok(true))
            && Instant::now() < deadline
        {
            assert!(
                game.try_wait().unwrap().is_none(),
                "fake game exited before detection"
            );
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(
            matches!(perfect_sync_core::process::try_is_running(), Ok(true)),
            "fake game was not detected"
        );

        let injector = perfect_sync_core::process::command(&executable)
            .env(CONSOLE_INJECTOR, target.id().to_string())
            .args(["--exact", test_name])
            .output()
            .unwrap();
        let _ = game.kill();
        let _ = game.wait();
        assert!(
            injector.status.success(),
            "{}{}",
            String::from_utf8_lossy(&injector.stdout),
            String::from_utf8_lossy(&injector.stderr)
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
}

#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
