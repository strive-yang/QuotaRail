#![cfg(all(windows, feature = "test-support"))]
mod p1_common;

use p1_common::Temp;
use quotarail::desktop::windows::{CMD_EXIT, CMD_REFRESH, wide, window_class};
use std::{
    mem::{size_of, zeroed},
    path::Path,
    process::{Child, Command, Stdio},
    ptr::{null, null_mut},
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, HWND, WAIT_TIMEOUT},
    System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject},
    UI::{Shell::*, WindowsAndMessaging::*},
};

struct App(Child);
impl Drop for App {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn launch(root: &Path, scenario: &str) -> App {
    App(Command::new(env!("CARGO_BIN_EXE_quotarail"))
        .args([
            "--tray-only",
            "--codex",
            env!("CARGO_BIN_EXE_mock_codex"),
            "--codex-home",
        ])
        .arg(root)
        .arg("--data-dir")
        .arg(root)
        .env("QUOTARAIL_MOCK_SCENARIO", scenario)
        .env("QUOTARAIL_MOCK_DELAY_MS", "500")
        .env("QUOTARAIL_MOCK_REQUEST_LOG", root.join("requests.log"))
        .env("QUOTARAIL_MOCK_PID_FILE", root.join("pids.txt"))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap())
}
#[track_caller]
fn until(mut check: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(10);
    while !check() {
        assert!(Instant::now() < end, "native tray operation timed out");
        std::thread::sleep(Duration::from_millis(20));
    }
}
fn window(root: &Path) -> HWND {
    unsafe { FindWindowW(wide(&window_class(root)).as_ptr(), null()) }
}
fn caption(hwnd: HWND) -> String {
    let mut text = [0u16; 256];
    let length = unsafe { GetWindowTextW(hwnd, text.as_mut_ptr(), text.len() as i32) };
    String::from_utf16_lossy(&text[..length.max(0) as usize])
}
fn post(hwnd: HWND, message: u32, value: usize) {
    assert_ne!(unsafe { PostMessageW(hwnd, message, value, 0) }, 0);
}
fn log(root: &Path) -> String {
    std::fs::read_to_string(root.join("requests.log")).unwrap_or_default()
}
fn count(root: &Path, method: &str) -> usize {
    log(root)
        .lines()
        .filter(|line| line.ends_with(&format!(" {method}")))
        .count()
}
fn running(pid: u32) -> bool {
    let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
    if handle.is_null() {
        return false;
    }
    let alive = unsafe { WaitForSingleObject(handle, 0) } == WAIT_TIMEOUT;
    unsafe {
        CloseHandle(handle);
    }
    alive
}
fn icon_registered(hwnd: HWND) -> bool {
    // A registered icon may live in the collapsed overflow area and have no visible rect.
    // A zero-flags MODIFY checks existence without changing its content or user preference.
    let mut data: NOTIFYICONDATAW = unsafe { zeroed() };
    data.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
    data.hWnd = hwnd;
    data.uID = 1;
    unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) != 0 }
}
fn stop(app: &mut App, hwnd: HWND) {
    post(hwnd, WM_COMMAND, CMD_EXIT as usize);
    until(|| app.0.try_wait().unwrap().is_some());
    assert!(app.0.wait().unwrap().success());
}

// Runs on the current interactive Windows desktop, with only synthetic quota data.
#[test]
fn native_tray_single_instance_refresh_readd_and_exit() {
    let temp = Temp::new();
    let mut app = launch(&temp.0, "descendant");
    until(|| !window(&temp.0).is_null());
    let hwnd = window(&temp.0);
    for _ in 0..40 {
        post(hwnd, WM_COMMAND, CMD_REFRESH as usize);
    }
    until(|| caption(hwnd).contains("正常"));
    until(|| icon_registered(hwnd));
    assert_eq!(count(&temp.0, "initialize"), 1);
    assert_eq!(count(&temp.0, "account/rateLimits/read"), 1);
    let pids: Vec<u32> = std::fs::read_to_string(temp.0.join("pids.txt"))
        .unwrap()
        .split_whitespace()
        .map(|s| s.parse().unwrap())
        .collect();
    let cached = std::fs::read_dir(&temp.0)
        .unwrap()
        .flatten()
        .find(|e| e.file_name().to_string_lossy().starts_with("quota-"))
        .unwrap();
    let body = std::fs::read_to_string(cached.path()).unwrap();
    for secret in [
        "SYNTHETIC_PRIVATE_ACCOUNT",
        "SYNTHETIC_PRIVATE_EMAIL",
        "SYNTHETIC_SECRET_STDERR",
    ] {
        assert!(!body.contains(secret));
    }

    let mut second = launch(&temp.0, "descendant");
    until(|| second.0.try_wait().unwrap().is_some());
    assert!(second.0.wait().unwrap().success());
    let mut dialog = null_mut();
    until(|| {
        dialog = unsafe {
            FindWindowW(
                wide("#32770").as_ptr(),
                wide("QuotaRail · 额度详情").as_ptr(),
            )
        };
        let mut owner_pid = 0;
        if !dialog.is_null() {
            unsafe {
                GetWindowThreadProcessId(dialog, &mut owner_pid);
            }
        }
        owner_pid == app.0.id() && unsafe { GetWindow(dialog, GW_OWNER) } == hwnd
    });
    post(dialog, WM_CLOSE, 0);
    assert_eq!(count(&temp.0, "initialize"), 1);

    // Delete only our icon, then exercise TaskbarCreated; never restart the user's Explorer.
    let mut own_icon: NOTIFYICONDATAW = unsafe { zeroed() };
    own_icon.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
    own_icon.hWnd = hwnd;
    own_icon.uID = 1;
    assert_ne!(unsafe { Shell_NotifyIconW(NIM_DELETE, &own_icon) }, 0);
    let broadcast = unsafe { RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()) };
    assert_ne!(broadcast, 0);
    post(hwnd, broadcast, 0);
    until(|| icon_registered(hwnd));

    // Exercise the real 30-second cooldown, then verify reuse of the same peer.
    let start = Instant::now();
    until(|| !caption(hwnd).contains("刷新中"));
    while start.elapsed() < Duration::from_secs(31) {
        std::thread::sleep(Duration::from_millis(100));
    }
    post(hwnd, WM_COMMAND, CMD_REFRESH as usize);
    until(|| count(&temp.0, "account/rateLimits/read") == 2);
    until(|| caption(hwnd).contains("正常"));
    assert_eq!(count(&temp.0, "initialize"), 1);
    assert!(
        log(&temp.0)
            .lines()
            .all(|line| line.starts_with(&format!("{} ", pids[0])))
    );
    stop(&mut app, hwnd);
    until(|| pids.iter().all(|pid| !running(*pid)));
}

#[test]
fn signed_out_tray_keeps_no_current_quota_and_can_exit() {
    let temp = Temp::new();
    let mut app = launch(&temp.0, "signed_out");
    until(|| caption(window(&temp.0)).contains("未登录"));
    let hwnd = window(&temp.0);
    assert_eq!(count(&temp.0, "account/rateLimits/read"), 0);
    assert!(
        !std::fs::read_dir(&temp.0)
            .unwrap()
            .flatten()
            .any(|e| e.file_name().to_string_lossy().starts_with("quota-"))
    );
    stop(&mut app, hwnd);
}

#[test]
fn exit_during_inflight_read_cleans_owned_job() {
    let temp = Temp::new();
    let mut app = launch(&temp.0, "descendant");
    until(|| count(&temp.0, "account/rateLimits/read") == 1);
    let pids: Vec<u32> = std::fs::read_to_string(temp.0.join("pids.txt"))
        .unwrap()
        .split_whitespace()
        .map(|s| s.parse().unwrap())
        .collect();
    stop(&mut app, window(&temp.0));
    until(|| pids.iter().all(|pid| !running(*pid)));
}
