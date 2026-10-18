#![cfg(all(windows, feature = "test-support"))]
mod p1_common;
use p1_common::Temp;
use quotarail::desktop::{
    preferences::{DisplayMode, Preferences},
    settings::*,
    widget::rail_class,
    widget_layout::{Theme, WidgetConfig},
    windows::{self, wide},
};
use std::{
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    ptr::{null, null_mut},
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::*,
    System::{Registry::*, Threading::*},
    UI::WindowsAndMessaging::*,
};

#[test]
fn desktop_binary_embeds_standard_user_and_per_monitor_manifest() {
    use windows_sys::Win32::System::LibraryLoader::*;
    let module = unsafe {
        LoadLibraryExW(
            wide(env!("CARGO_BIN_EXE_quotarail")).as_ptr(),
            null_mut(),
            LOAD_LIBRARY_AS_DATAFILE | LOAD_LIBRARY_AS_IMAGE_RESOURCE,
        )
    };
    assert!(!module.is_null());
    struct Module(HMODULE);
    impl Drop for Module {
        fn drop(&mut self) {
            unsafe {
                FreeLibrary(self.0);
            }
        }
    }
    let _module = Module(module);
    // Win32 integer resource identifiers are encoded as pointer-sized values,
    // not dereferenced addresses (MAKEINTRESOURCE semantics).
    let resource = unsafe { FindResourceW(module, std::ptr::without_provenance(1), RT_MANIFEST) };
    assert!(!resource.is_null());
    let length = unsafe { SizeofResource(module, resource) } as usize;
    let data = unsafe { LockResource(LoadResource(module, resource)) } as *const u8;
    assert!(!data.is_null());
    let bytes = unsafe { std::slice::from_raw_parts(data, length) };
    let xml = String::from_utf8_lossy(bytes);
    assert!(
        xml.contains("asInvoker")
            && xml.contains("PerMonitorV2")
            && xml.contains("Microsoft.Windows.Common-Controls")
    );
}
static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
struct App {
    child: Child,
    root: PathBuf,
}
impl Drop for App {
    fn drop(&mut self) {
        let h = find(&windows::window_class(&self.root));
        if !h.is_null() {
            unsafe {
                PostMessageW(h, WM_COMMAND, windows::CMD_EXIT as usize, 0);
            }
        }
        for _ in 0..40 {
            if self.child.try_wait().unwrap_or_default().is_some() {
                return;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
struct Registry(String);
impl Drop for Registry {
    fn drop(&mut self) {
        unsafe {
            RegDeleteKeyW(HKEY_CURRENT_USER, wide(&self.0).as_ptr());
        }
    }
}
fn launch(root: &Path, key: &str, saved: bool) -> App {
    let mut c = Command::new(env!("CARGO_BIN_EXE_quotarail"));
    c.arg("--data-dir").arg(root);
    if !saved {
        c.args(["--codex", env!("CARGO_BIN_EXE_mock_codex"), "--codex-home"])
            .arg(root);
    }
    App {
        child: c
            .env("QUOTARAIL_MOCK_SCENARIO", "widget")
            .env("QUOTARAIL_MOCK_DELAY_MS", "500")
            .env("QUOTARAIL_MOCK_REQUEST_LOG", root.join("requests.log"))
            .env("QUOTARAIL_TEST_STARTUP_KEY", key)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
        root: root.to_owned(),
    }
}
#[track_caller]
fn until(mut f: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(12);
    while !f() {
        assert!(Instant::now() < end, "P3 native operation timed out");
        std::thread::sleep(Duration::from_millis(20));
    }
}
fn find(c: &str) -> HWND {
    unsafe { FindWindowW(wide(c).as_ptr(), null()) }
}
fn visible(h: HWND) -> bool {
    unsafe { IsWindowVisible(h) != 0 }
}
fn text(h: HWND) -> String {
    let mut b = [0u16; 8192];
    let n = unsafe { GetWindowTextW(h, b.as_mut_ptr(), b.len() as i32) };
    String::from_utf16_lossy(&b[..n.max(0) as usize])
}
fn send(h: HWND, m: u32, w: usize, l: isize) -> usize {
    let mut result = 0;
    assert_ne!(
        unsafe { SendMessageTimeoutW(h, m, w, l, SMTO_ABORTIFHUNG, 2000, &mut result) },
        0
    );
    result
}
fn post(h: HWND, m: u32, w: usize) {
    assert_ne!(unsafe { PostMessageW(h, m, w, 0) }, 0);
}
fn set(h: HWND, id: i32, value: &str) {
    send(
        unsafe { GetDlgItem(h, id) },
        WM_SETTEXT,
        0,
        wide(value).as_ptr() as isize,
    );
}
fn choose(h: HWND, id: i32, index: usize) {
    send(unsafe { GetDlgItem(h, id) }, CB_SETCURSEL, index, 0);
}
fn check(h: HWND, id: i32, value: bool) {
    send(
        unsafe { GetDlgItem(h, id) },
        BM_SETCHECK,
        usize::from(value),
        0,
    );
}
fn log(root: &Path) -> String {
    std::fs::read_to_string(root.join("requests.log")).unwrap_or_default()
}
fn reads(root: &Path) -> usize {
    log(root)
        .lines()
        .filter(|l| l.ends_with(" account/rateLimits/read"))
        .count()
}
fn open(owner: HWND, root: &Path) -> HWND {
    post(owner, WM_COMMAND, CMD_SETTINGS as usize);
    until(|| visible(find(&class(root))));
    find(&class(root))
}
fn save(h: HWND) {
    post(h, WM_COMMAND, 1);
    until(|| !visible(h));
}
fn startup(key: &str) -> Option<String> {
    let mut b = [0u16; 8192];
    let mut n = (b.len() * 2) as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            wide(key).as_ptr(),
            wide("QuotaRail").as_ptr(),
            RRF_RT_REG_SZ,
            null_mut(),
            b.as_mut_ptr().cast(),
            &mut n,
        )
    };
    if status == ERROR_FILE_NOT_FOUND {
        return None;
    }
    assert_eq!(status, ERROR_SUCCESS);
    Some(String::from_utf16_lossy(&b[..n as usize / 2 - 1]))
}
fn alive(pid: u32) -> bool {
    let h = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
    if h.is_null() {
        return false;
    }
    let result = unsafe { WaitForSingleObject(h, 0) };
    unsafe {
        CloseHandle(h);
    }
    result == WAIT_TIMEOUT
}
fn stop(app: &mut App, owner: HWND) {
    post(owner, WM_COMMAND, windows::CMD_EXIT as usize);
    until(|| app.child.try_wait().unwrap().is_some());
    assert!(app.child.wait().unwrap().success());
}
fn child_rail(root: &Path) -> HWND {
    unsafe {
        let host = FindWindowW(wide("Shell_TrayWnd").as_ptr(), null());
        FindWindowExW(host, null_mut(), wide(&rail_class(root)).as_ptr(), null())
    }
}

#[test]
fn settings_apply_without_duplicate_reads_and_source_changes_retire_the_old_session() {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let temp = Temp::new();
    let key = Registry(format!(
        "Software\\QuotaRail\\Tests\\Settings-{}",
        std::process::id()
    ));
    let mut app = launch(&temp.0, &key.0, false);
    let owner_class = windows::window_class(&temp.0);
    until(|| text(find(&owner_class)).contains("正常"));
    let owner = find(&owner_class);
    let old_pid = log(&temp.0)
        .lines()
        .find(|l| l.ends_with(" initialize"))
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .parse::<u32>()
        .unwrap();
    assert!(startup(&key.0).is_none());
    let h = open(owner, &temp.0);
    assert_eq!(unsafe { GetWindow(h, GW_OWNER) }, owner);
    set(h, ID_REFRESH, "10");
    choose(h, ID_THEME, 2);
    check(h, ID_LOCKED, true);
    check(h, ID_STARTUP, true);
    save(h);
    until(|| Preferences::load(&temp.0).unwrap().refresh_minutes == 10);
    assert_eq!(WidgetConfig::load(&temp.0).unwrap().theme, Theme::Dark);
    assert!(WidgetConfig::load(&temp.0).unwrap().locked);
    let command = startup(&key.0).unwrap();
    assert!(command.contains("--data-dir") && command.contains(temp.0.to_str().unwrap()));
    assert_eq!(reads(&temp.0), 1);
    assert!(alive(old_pid));
    let h = open(owner, &temp.0);
    choose(h, ID_MODE, 1);
    check(h, ID_STARTUP, false);
    save(h);
    until(|| !find(&rail_class(&temp.0)).is_null());
    assert!(child_rail(&temp.0).is_null());
    assert!(startup(&key.0).is_none());
    assert_eq!(reads(&temp.0), 1);
    assert!(alive(old_pid));
    let before = std::fs::read(temp.0.join("settings.json")).unwrap();
    let h = open(owner, &temp.0);
    set(h, ID_REFRESH, "3");
    post(h, WM_COMMAND, 1);
    let mut error = null_mut();
    until(|| {
        error = unsafe {
            FindWindowW(
                wide("#32770").as_ptr(),
                wide("QuotaRail · 设置提示").as_ptr(),
            )
        };
        visible(error)
    });
    post(error, WM_CLOSE, 0);
    until(|| unsafe { IsWindow(error) == 0 });
    assert!(visible(h));
    post(unsafe { GetDlgItem(h, ID_REFRESH) }, WM_KEYDOWN, 0x1B);
    until(|| !visible(h));
    assert_eq!(std::fs::read(temp.0.join("settings.json")).unwrap(), before);
    let new_home = temp.0.join("SYNTHETIC Other Home");
    std::fs::create_dir(&new_home).unwrap();
    let h = open(owner, &temp.0);
    set(h, ID_HOME, new_home.to_str().unwrap());
    save(h);
    assert!(
        !text(find(&rail_class(&temp.0))).contains("58.0%"),
        "old source data must be cleared before the new response"
    );
    until(|| reads(&temp.0) == 2 && text(owner).contains("正常"));
    assert!(!alive(old_pid));
    assert_eq!(
        Preferences::load(&temp.0).unwrap().codex_home,
        Some(new_home)
    );
    let h = open(owner, &temp.0);
    choose(h, ID_MODE, 2);
    save(h);
    until(|| find(&rail_class(&temp.0)).is_null() && child_rail(&temp.0).is_null());
    assert_eq!(reads(&temp.0), 2);
    let h = open(owner, &temp.0);
    choose(h, ID_MODE, 0);
    save(h);
    until(|| !child_rail(&temp.0).is_null());
    assert_eq!(reads(&temp.0), 2);
    stop(&mut app, owner);
    let mut again = launch(&temp.0, &key.0, true);
    until(|| text(find(&owner_class)).contains("正常"));
    assert_eq!(
        Preferences::load(&temp.0).unwrap().mode,
        DisplayMode::Taskbar
    );
    assert!(!child_rail(&temp.0).is_null());
    assert_eq!(reads(&temp.0), 3);
    stop(&mut again, find(&owner_class));
}

#[test]
fn damaged_settings_are_not_overwritten_on_open_or_cancel() {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let temp = Temp::new();
    let key = Registry(format!(
        "Software\\QuotaRail\\Tests\\Recovery-{}",
        std::process::id()
    ));
    let invalid = br#"{"schema_version":99,"future":"SYNTHETIC_NOT_A_TOKEN"}"#;
    std::fs::write(temp.0.join("settings.json"), invalid).unwrap();
    let mut app = launch(&temp.0, &key.0, false);
    let owner_class = windows::window_class(&temp.0);
    until(|| text(find(&owner_class)).contains("正常"));
    let owner = find(&owner_class);
    let h = open(owner, &temp.0);
    post(h, WM_COMMAND, 2);
    until(|| !visible(h));
    assert_eq!(
        std::fs::read(temp.0.join("settings.json")).unwrap(),
        invalid
    );
    assert!(startup(&key.0).is_none());
    let h = open(owner, &temp.0);
    post(unsafe { GetDlgItem(h, ID_REFRESH) }, WM_KEYDOWN, 0x0D);
    until(|| !visible(h));
    assert_eq!(Preferences::load(&temp.0).unwrap().schema_version, 1);
    assert_eq!(reads(&temp.0), 1);
    stop(&mut app, owner);
}
