#![cfg(all(windows, feature = "test-support"))]
mod p1_common;
use p1_common::Temp;
use quotarail::desktop::{
    widget::*,
    widget_layout::*,
    windows::{self, wide},
};
use std::{
    path::Path,
    process::{Child, Command, Stdio},
    ptr::{null, null_mut},
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{HWND, POINT, RECT},
    Graphics::Gdi::*,
    UI::{HiDpi::*, Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
};
static UI_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
struct App(Child);
impl Drop for App {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
struct Cursor(POINT);
impl Cursor {
    fn save() -> Self {
        let mut p = POINT { x: 0, y: 0 };
        unsafe {
            GetCursorPos(&mut p);
        }
        Self(p)
    }
}
impl Drop for Cursor {
    fn drop(&mut self) {
        unsafe {
            SetCursorPos(self.0.x, self.0.y);
        }
    }
}
struct Dpi(isize);
impl Drop for Dpi {
    fn drop(&mut self) {
        unsafe {
            SetThreadDpiAwarenessContext(self.0 as _);
        }
    }
}
struct TestWindow(HWND);
impl Drop for TestWindow {
    fn drop(&mut self) {
        unsafe {
            DestroyWindow(self.0);
        }
    }
}
fn outside_click_is_forwarded(card: HWND, work: RECT) {
    let _cursor = Cursor::save();
    unsafe extern "system" fn target_proc(h: HWND, m: u32, w: usize, l: isize) -> isize {
        if m == WM_MOUSEACTIVATE {
            MA_NOACTIVATE as isize
        } else {
            unsafe { DefWindowProcW(h, m, w, l) }
        }
    }
    let class = wide(&format!("QuotaRail.TestClickTarget.{}", std::process::id()));
    let wc = WNDCLASSW {
        lpszClassName: class.as_ptr(),
        lpfnWndProc: Some(target_proc),
        ..unsafe { std::mem::zeroed() }
    };
    assert_ne!(unsafe { RegisterClassW(&wc) }, 0);
    let h = unsafe {
        CreateWindowExW(
            WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
            class.as_ptr(),
            wide("SYNTHETIC_P2_CLICK_TARGET").as_ptr(),
            WS_POPUP | WS_VISIBLE,
            work.left + 20,
            work.top + 20,
            60,
            40,
            null_mut(),
            null_mut(),
            null_mut(),
            null(),
        )
    };
    assert!(!h.is_null());
    let _target = TestWindow(h);
    unsafe {
        SetWindowPos(
            h,
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
        SetCursorPos(work.left + 40, work.top + 40);
    }
    assert!(visible(card));
    assert_eq!(
        unsafe {
            WindowFromPoint(POINT {
                x: work.left + 40,
                y: work.top + 40,
            })
        },
        h
    );
    let input = [MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP].map(|flags| INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    });
    assert_eq!(
        unsafe {
            SendInput(
                input.len() as u32,
                input.as_ptr(),
                std::mem::size_of::<INPUT>() as i32,
            )
        },
        2
    );
    until(|| !visible(card));
    let mut message = unsafe { std::mem::zeroed() };
    until(|| unsafe {
        PeekMessageW(&mut message, h, WM_LBUTTONDOWN, WM_LBUTTONDOWN, PM_REMOVE) != 0
    });
}
fn launch(root: &Path) -> App {
    launch_mode(root, false)
}
// A background runner cannot reliably activate Explorer. First receive real
// input in our own temporary window; this gives this test a legitimate foreground
// transition and grants the synthetic app permission to activate its detail card.
fn activate_taskbar(host: HWND, app_pid: u32) {
    let _window = activate_fixture(host, Some(app_pid));
}
fn activate_fixture(host: HWND, activate_bar_for: Option<u32>) -> TestWindow {
    let _cursor = Cursor::save();
    let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    unsafe {
        GetMonitorInfoW(MonitorFromWindow(host, MONITOR_DEFAULTTOPRIMARY), &mut info);
    }
    unsafe extern "system" fn activation_proc(h: HWND, m: u32, w: usize, l: isize) -> isize {
        if m == WM_LBUTTONUP {
            unsafe {
                SetPropW(h, wide("QuotaRail.TestClickFinished").as_ptr(), 1usize as _);
            }
        }
        unsafe { DefWindowProcW(h, m, w, l) }
    }
    let class = wide(&format!("QuotaRail.TestActivation.{}", std::process::id()));
    let wc = WNDCLASSW {
        lpszClassName: class.as_ptr(),
        lpfnWndProc: Some(activation_proc),
        ..unsafe { std::mem::zeroed() }
    };
    let registered = unsafe { RegisterClassW(&wc) };
    assert!(
        registered != 0
            || unsafe { windows_sys::Win32::Foundation::GetLastError() }
                == windows_sys::Win32::Foundation::ERROR_CLASS_ALREADY_EXISTS
    );
    let h = unsafe {
        CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            class.as_ptr(),
            wide("SYNTHETIC QuotaRail activation target").as_ptr(),
            WS_POPUP | WS_VISIBLE,
            info.rcWork.left + 20,
            info.rcWork.top + 20,
            80,
            40,
            null_mut(),
            null_mut(),
            null_mut(),
            null(),
        )
    };
    assert!(!h.is_null());
    let window = TestWindow(h);
    unsafe {
        SetCursorPos(info.rcWork.left + 40, info.rcWork.top + 40);
    }
    assert_eq!(
        unsafe {
            WindowFromPoint(POINT {
                x: info.rcWork.left + 40,
                y: info.rcWork.top + 40,
            })
        },
        h,
        "activation input must target our own fixture"
    );
    let input = [MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP].map(|flags| INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    });
    assert_eq!(
        unsafe { SendInput(2, input.as_ptr(), std::mem::size_of::<INPUT>() as i32) },
        2
    );
    let mut msg = unsafe { std::mem::zeroed() };
    until(|| unsafe {
        while PeekMessageW(&mut msg, null_mut(), 0, 0, PM_REMOVE) != 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        GetForegroundWindow() == h
            && GetPropW(h, wide("QuotaRail.TestClickFinished").as_ptr()) as usize == 1
    });
    if let Some(app_pid) = activate_bar_for {
        assert_ne!(unsafe { AllowSetForegroundWindow(app_pid) }, 0);
        assert_ne!(unsafe { SetForegroundWindow(host) }, 0);
    }
    window
}
fn launch_mode(root: &Path, taskbar: bool) -> App {
    launch_flags(root, if taskbar { &[] } else { &["--floating"] })
}
fn launch_flags(root: &Path, flags: &[&str]) -> App {
    let mut command = Command::new(env!("CARGO_BIN_EXE_quotarail"));
    command.args(flags);
    App(command
        .args(["--codex", env!("CARGO_BIN_EXE_mock_codex"), "--codex-home"])
        .arg(root)
        .arg("--data-dir")
        .arg(root)
        .env("QUOTARAIL_MOCK_SCENARIO", "widget")
        .env("QUOTARAIL_MOCK_REQUEST_LOG", root.join("requests.log"))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap())
}

// Do not activate Explorer or open details before the startup assertion: both
// actions in the older test can repair the very ordering bug being measured.
#[test]
fn embedded_taskbar_is_hittable_on_startup_and_after_other_window_activation() {
    let _guard = UI_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _dpi =
        Dpi(
            unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) }
                as isize,
        );
    let temp = Temp::new();
    let host = quotarail::desktop::taskbar::Host::discover(240).unwrap();
    let mut app = launch_mode(&temp.0, true);
    let owner_class = windows::window_class(&temp.0);
    until(|| text(find(&owner_class)).contains("正常"));
    let owner = find(&owner_class);
    let rail = unsafe {
        FindWindowExW(
            host.hwnd,
            null_mut(),
            wide(&rail_class(&temp.0)).as_ptr(),
            null(),
        )
    };
    assert!(!rail.is_null());
    let hit = || {
        let r = rect(rail);
        unsafe {
            WindowFromPoint(POINT {
                x: r.left + r.width() / 2,
                y: r.top + r.height() / 2,
            })
        }
    };
    // Read only four opaque pixels belonging to the synthetic rail, never
    // capture a desktop image. HWND visibility alone misses compositor occlusion.
    let painted = || {
        let r = rect(rail);
        let dpi = unsafe { GetDpiForWindow(rail) };
        unsafe {
            windows_sys::Win32::Graphics::Dwm::DwmFlush();
            let dc = GetDC(null_mut());
            assert!(!dc.is_null());
            let colors = [(44.0, 11.0), (49.0, 11.0), (44.0, 29.0), (49.0, 29.0)]
                .map(|(x, y)| GetPixel(dc, r.left + px(x, dpi), r.top + px(y, dpi)));
            ReleaseDC(null_mut(), dc);
            let blue = |c: u32| c == 0x00F88734 || c == 0x00FFA564;
            let green = |c: u32| c == 0x008DC824 || c == 0x00A2D942;
            (
                blue(colors[0]) && blue(colors[1]) && green(colors[2]) && green(colors[3]),
                colors,
            )
        }
    };
    let describe = |h| {
        let mut name = [0u16; 256];
        let n = unsafe { GetClassNameW(h, name.as_mut_ptr(), name.len() as i32) };
        String::from_utf16_lossy(&name[..n.max(0) as usize])
    };
    std::thread::sleep(Duration::from_millis(500));
    let cold = hit() == rail && painted().0;
    eprintln!(
        "cold_start: visible={cold}, hit={}, pixels={:x?}",
        describe(hit()),
        painted().1
    );
    // Opening via our tray command deliberately follows the user's recovery
    // action, then a real click transfers focus to an independent test window.
    let foreground = activate_fixture(host.hwnd, None);
    assert_ne!(unsafe { AllowSetForegroundWindow(app.0.id()) }, 0);
    post(owner, WM_COMMAND, windows::CMD_DETAILS as usize);
    let card = find(&card_class(&temp.0));
    until(|| visible(card));
    drop(foreground);
    let _other = activate_fixture(host.hwnd, None);
    until(|| !visible(card));
    let mut after = true;
    for _ in 0..10 {
        std::thread::sleep(Duration::from_millis(100));
        after &= hit() == rail && painted().0;
    }
    eprintln!(
        "after_other_activation: visible={after}, hit={}, pixels={:x?}, parent_is_taskbar={}",
        describe(hit()),
        painted().1,
        unsafe { GetParent(rail) } == host.hwnd
    );
    let _cursor = Cursor::save();
    let current = quotarail::desktop::taskbar::Host::discover(240).unwrap();
    let point = POINT {
        x: current.slot.right + px(4.0, current.dpi),
        y: rect(rail).top + px(24.0, current.dpi),
    };
    let target = unsafe { WindowFromPoint(point) };
    assert_eq!(
        unsafe { GetAncestor(target, GA_ROOT) },
        host.hwnd,
        "background click must target the verified taskbar gap"
    );
    assert_ne!(target, rail);
    unsafe {
        SetCursorPos(point.x, point.y);
    }
    let inputs = [MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP].map(|flags| INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dwFlags: flags,
                ..unsafe { std::mem::zeroed() }
            },
        },
    });
    assert_eq!(
        unsafe { SendInput(2, inputs.as_ptr(), std::mem::size_of::<INPUT>() as i32) },
        2
    );
    let mut background = true;
    for _ in 0..10 {
        std::thread::sleep(Duration::from_millis(100));
        background &= hit() == rail && painted().0;
    }
    eprintln!(
        "after_taskbar_background_click: visible={background}, hit={}, pixels={:x?}",
        describe(hit()),
        painted().1
    );
    stop(&mut app, owner);
    assert!(
        cold && after && background,
        "taskbar rail occluded: cold_start={cold}, after_other_activation={after}, after_taskbar_click={background}"
    );
}

#[test]
fn embedded_taskbar_survives_activation_details_rebind_and_clamped_drag() {
    let _guard = UI_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _dpi =
        Dpi(
            unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) }
                as isize,
        );
    let temp = Temp::new();
    let config = WidgetConfig {
        placement: Some(Placement {
            monitor: "SYNTHETIC_FLOATING_MONITOR".into(),
            anchor: Anchor::BottomLeft,
            x_dip: 123.0,
            y_dip: 45.0,
        }),
        ..Default::default()
    };
    config.save(&temp.0).unwrap();
    let mut app = launch_mode(&temp.0, true);
    until(|| text(find(&windows::window_class(&temp.0))).contains("正常"));
    let owner = find(&windows::window_class(&temp.0));
    let host = quotarail::desktop::taskbar::Host::discover(240).unwrap();
    let find_rail = || unsafe {
        FindWindowExW(
            host.hwnd,
            null_mut(),
            wide(&rail_class(&temp.0)).as_ptr(),
            null(),
        )
    };
    until(|| !find_rail().is_null() && visible(find_rail()));
    let mut rail = find_rail();
    let card = find(&card_class(&temp.0));
    assert_eq!(unsafe { GetParent(rail) }, host.hwnd);
    assert_eq!(
        unsafe { GetWindow(card, GW_OWNER) },
        owner,
        "popup must be owned by our hidden host, never Explorer"
    );
    assert_ne!(
        unsafe { GetWindowLongPtrW(rail, GWL_STYLE) } as u32 & WS_CHILD,
        0
    );
    assert_eq!(
        unsafe { GetWindowLongPtrW(rail, GWL_STYLE) } as u32 & WS_POPUP,
        0
    );
    assert_eq!(
        unsafe { GetWindowLongPtrW(rail, GWL_EXSTYLE) } as u32 & WS_EX_TOPMOST,
        0
    );
    let initial = rect(rail);
    assert_eq!(initial, host.position(0.0));
    until(|| text(rail).contains("5H 剩余 58.0%"));
    for _ in 0..3 {
        activate_taskbar(host.hwnd, app.0.id());
        real_drag(rail, 0, 0);
        until(|| visible(card));
        send(card, WM_KEYDOWN, VK_ESCAPE as usize, 0);
        until(|| !visible(card));
        assert_eq!(rect(rail), initial);
    }
    activate_taskbar(host.hwnd, app.0.id());
    real_drag(rail, 0, 0);
    until(|| visible(card));
    send(card, WM_KEYDOWN, VK_TAB as usize, 0);
    send(card, WM_KEYDOWN, VK_RETURN as usize, 0);
    let mut dialog = null_mut();
    until(|| {
        dialog = unsafe {
            FindWindowW(
                wide("#32770").as_ptr(),
                wide("QuotaRail · 额度详情").as_ptr(),
            )
        };
        !dialog.is_null() && visible(dialog)
    });
    let mut pid = 0;
    unsafe {
        GetWindowThreadProcessId(dialog, &mut pid);
    }
    assert_eq!(pid, app.0.id());
    assert_eq!(unsafe { GetWindow(dialog, GW_OWNER) }, owner);
    assert_ne!(
        unsafe { IsWindowEnabled(host.hwnd) },
        0,
        "modal details must never disable Explorer"
    );
    post(dialog, WM_CLOSE, 0);
    until(|| unsafe { IsWindow(dialog) == 0 });
    real_drag(rail, -px(1000.0, host.dpi), -px(200.0, host.dpi));
    assert_eq!(rect(rail).left, host.slot.left);
    assert_eq!(
        rect(rail).top,
        initial.top,
        "drag remains inside the taskbar slot"
    );
    until(|| WidgetConfig::load(&temp.0).unwrap().taskbar_offset_dip > 0.0);
    assert_eq!(
        WidgetConfig::load(&temp.0)
            .unwrap()
            .placement
            .unwrap()
            .x_dip,
        123.0,
        "taskbar drag must preserve floating placement"
    );
    for _ in 0..2 {
        let old_rail = rail;
        post(owner, WM_COMMAND, CMD_REBIND as usize);
        until(|| {
            !find_rail().is_null()
                && find_rail() != old_rail
                && visible(find_rail())
                && text(find_rail()).contains("剩余")
        });
        rail = find_rail();
        until(|| {
            rect(rail)
                == quotarail::desktop::taskbar::Host::discover(240)
                    .unwrap()
                    .position(WidgetConfig::load(&temp.0).unwrap().taskbar_offset_dip)
        });
        assert_eq!(unsafe { GetParent(rail) }, host.hwnd);
        activate_taskbar(host.hwnd, app.0.id());
        real_drag(rail, 0, 0);
        until(|| visible(card));
        send(card, WM_KEYDOWN, VK_ESCAPE as usize, 0);
        until(|| !visible(card));
    }
    assert_eq!(
        reads(&temp.0),
        1,
        "remount must not start duplicate quota workers"
    );
    stop(&mut app, owner);
    assert!(find_rail().is_null());
    let mut again = launch_mode(&temp.0, true);
    until(|| !find_rail().is_null() && visible(find_rail()));
    // App-button animation can move Start during relaunch. Restore the saved
    // offset relative to the current slot, not a stale absolute screen pixel.
    until(|| {
        rect(find_rail())
            == quotarail::desktop::taskbar::Host::discover(240)
                .unwrap()
                .position(WidgetConfig::load(&temp.0).unwrap().taskbar_offset_dip)
    });
    let owner = find(&windows::window_class(&temp.0));
    post(owner, WM_COMMAND, CMD_RESET as usize);
    until(|| WidgetConfig::load(&temp.0).unwrap().taskbar_offset_dip == 0.0);
    until(|| {
        rect(find_rail())
            == quotarail::desktop::taskbar::Host::discover(240)
                .unwrap()
                .position(0.0)
    });
    assert_eq!(
        WidgetConfig::load(&temp.0)
            .unwrap()
            .placement
            .unwrap()
            .x_dip,
        123.0
    );
    stop(&mut again, owner);
    let mut fallback = launch_flags(&temp.0, &["--taskbar", "--taskbar-left-space", "1024"]);
    until(|| text(find(&windows::window_class(&temp.0))).contains("正常"));
    assert!(
        find_rail().is_null(),
        "a crowded layout must not attach over system controls"
    );
    let detached = find(&rail_class(&temp.0));
    assert!(
        !detached.is_null() && !visible(detached),
        "failed attachment remains hidden with the tray available"
    );
    stop(&mut fallback, find(&windows::window_class(&temp.0)));
}
#[track_caller]
fn until(mut f: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(10);
    while !f() {
        assert!(Instant::now() < end, "P2 native operation timed out");
        std::thread::sleep(Duration::from_millis(20));
    }
}
fn find(class: &str) -> HWND {
    unsafe { FindWindowW(wide(class).as_ptr(), null()) }
}
fn text(hwnd: HWND) -> String {
    let mut buf = [0u16; 4096];
    let n = unsafe { GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}
fn send(hwnd: HWND, message: u32, wp: usize, lp: isize) -> usize {
    let mut result = 0;
    assert_ne!(
        unsafe { SendMessageTimeoutW(hwnd, message, wp, lp, SMTO_ABORTIFHUNG, 2000, &mut result) },
        0
    );
    result
}
fn post(hwnd: HWND, message: u32, wp: usize) {
    assert_ne!(unsafe { PostMessageW(hwnd, message, wp, 0) }, 0);
}
fn visible(hwnd: HWND) -> bool {
    unsafe { IsWindowVisible(hwnd) != 0 }
}
fn rect(hwnd: HWND) -> Rect {
    let mut r = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    assert_ne!(unsafe { GetWindowRect(hwnd, &mut r) }, 0);
    Rect {
        left: r.left,
        top: r.top,
        right: r.right,
        bottom: r.bottom,
    }
}
fn reads(root: &Path) -> usize {
    std::fs::read_to_string(root.join("requests.log"))
        .unwrap_or_default()
        .lines()
        .filter(|s| s.ends_with(" account/rateLimits/read"))
        .count()
}
fn click(hwnd: HWND) {
    send(hwnd, WM_LBUTTONDOWN, 0, 0);
    send(hwnd, WM_LBUTTONUP, 0, 0);
}
fn drag(hwnd: HWND, dx: i32, dy: i32) {
    let _cursor = Cursor::save();
    let before = rect(hwnd);
    unsafe {
        SetCursorPos(before.left + 15, before.top + 15);
    }
    send(hwnd, WM_LBUTTONDOWN, 0, 0);
    unsafe {
        SetCursorPos(before.left + 15 + dx, before.top + 15 + dy);
    }
    send(hwnd, WM_MOUSEMOVE, 1, 0);
    send(hwnd, WM_LBUTTONUP, 0, 0);
}
#[track_caller]
fn real_drag(hwnd: HWND, dx: i32, dy: i32) {
    let _cursor = Cursor::save();
    let before = rect(hwnd);
    let start = POINT {
        x: before.left + before.width() / 2,
        y: before.top + before.height() / 2,
    };
    unsafe {
        SetCursorPos(start.x, start.y);
    }
    let hit = unsafe { WindowFromPoint(start) };
    let mut name = [0u16; 128];
    let n = unsafe { GetClassNameW(hit, name.as_mut_ptr(), name.len() as i32) };
    assert_eq!(
        hit,
        hwnd,
        "input must target our test rail; actual class={}, rail ex_style={:#x}",
        String::from_utf16_lossy(&name[..n.max(0) as usize]),
        unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) }
    );
    let input = |flags| INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    assert_eq!(
        unsafe {
            SendInput(
                1,
                &input(MOUSEEVENTF_LEFTDOWN),
                std::mem::size_of::<INPUT>() as i32,
            )
        },
        1
    );
    let thread = unsafe { GetWindowThreadProcessId(hwnd, null_mut()) };
    let captured = || unsafe {
        let mut info: GUITHREADINFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<GUITHREADINFO>() as u32;
        assert_ne!(GetGUIThreadInfo(thread, &mut info), 0);
        info.hwndCapture == hwnd
    };
    until(captured);
    unsafe {
        SetCursorPos(start.x + dx, start.y + dy);
    }
    if dx != 0 || dy != 0 {
        until(|| rect(hwnd) != before);
    }
    assert_eq!(
        unsafe {
            SendInput(
                1,
                &input(MOUSEEVENTF_LEFTUP),
                std::mem::size_of::<INPUT>() as i32,
            )
        },
        1
    );
    until(|| !captured());
}
fn stop(app: &mut App, owner: HWND) {
    post(owner, WM_COMMAND, windows::CMD_EXIT as usize);
    until(|| app.0.try_wait().unwrap().is_some());
    assert!(app.0.wait().unwrap().success());
}

#[test]
fn rail_card_drag_lock_restore_theme_visibility_and_restart() {
    let _guard = UI_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _dpi =
        Dpi(
            unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) }
                as isize,
        );
    let temp = Temp::new();
    let mut app = launch(&temp.0);
    until(|| text(find(&windows::window_class(&temp.0))).contains("正常"));
    let owner = find(&windows::window_class(&temp.0));
    let rail = find(&rail_class(&temp.0));
    let card = find(&card_class(&temp.0));
    assert!(!rail.is_null() && !card.is_null());
    assert_eq!(unsafe { GetWindow(card, GW_OWNER) }, rail);
    assert_ne!(
        unsafe { GetWindowLongPtrW(rail, GWL_EXSTYLE) } & WS_EX_LAYERED as isize,
        0
    );
    assert_eq!(
        unsafe { GetWindowLongPtrW(rail, GWL_STYLE) } & WS_THICKFRAME as isize,
        0
    );
    until(|| visible(rail));
    assert!(!visible(card));
    let initial = rect(rail);
    let dpi = unsafe { GetDpiForWindow(rail) };
    assert_eq!(initial.width(), px(RAIL_WIDTH, dpi));
    assert_eq!(initial.height(), px(RAIL_HEIGHT, dpi));
    assert_ne!(
        unsafe {
            WindowFromPoint(POINT {
                x: initial.left + 1,
                y: initial.top + 1,
            })
        },
        rail,
        "transparent border must pass input through"
    );
    assert_eq!(send(rail, WM_MOUSEACTIVATE, 0, 0), MA_NOACTIVATE as usize);
    let mut monitor: MONITORINFO = unsafe { std::mem::zeroed() };
    monitor.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    assert_ne!(
        unsafe {
            GetMonitorInfoW(
                MonitorFromWindow(rail, MONITOR_DEFAULTTONEAREST),
                &mut monitor,
            )
        },
        0
    );
    assert!(initial.left >= monitor.rcWork.left && initial.bottom <= monitor.rcWork.bottom);
    assert!(text(rail).contains("5H 剩余 58.0%") && text(rail).contains("7D 剩余 32.0%"));
    click(rail);
    until(|| visible(card));
    outside_click_is_forwarded(card, monitor.rcWork);
    click(rail);
    until(|| visible(card));
    assert!(
        text(card).contains("上次成功")
            && text(card).contains("重置")
            && text(card).contains("来源")
    );
    let cr = rect(card);
    assert!(cr.left >= monitor.rcWork.left && cr.bottom <= monitor.rcWork.bottom);
    send(card, WM_KEYDOWN, VK_TAB as usize, 0);
    send(card, WM_ACTIVATE, WA_INACTIVE as usize, 0);
    until(|| !visible(card));
    click(rail);
    until(|| visible(card));
    send(card, WM_KEYDOWN, VK_ESCAPE as usize, 0);
    until(|| !visible(card));
    assert_eq!(reads(&temp.0), 1);

    drag(rail, -px(180.0, dpi), -px(100.0, dpi));
    until(|| temp.0.join("widget.json").exists());
    let moved = rect(rail);
    assert_ne!(moved, initial);
    assert!(WidgetConfig::load(&temp.0).unwrap().placement.is_some());
    post(owner, WM_COMMAND, CMD_LOCK as usize);
    until(|| WidgetConfig::load(&temp.0).unwrap().locked);
    drag(rail, 100, 70);
    assert_eq!(rect(rail), moved);
    send(card, WM_KEYDOWN, VK_ESCAPE as usize, 0);
    post(owner, WM_COMMAND, CMD_THEME_DARK as usize);
    until(|| WidgetConfig::load(&temp.0).unwrap().theme == Theme::Dark);
    post(owner, WM_COMMAND, CMD_VISIBLE as usize);
    until(|| !visible(rail));
    assert!(!visible(card));
    post(owner, WM_COMMAND, CMD_VISIBLE as usize);
    until(|| visible(rail));
    assert_eq!(reads(&temp.0), 1);
    stop(&mut app, owner);

    let mut again = launch(&temp.0);
    until(|| text(find(&windows::window_class(&temp.0))).contains("正常"));
    let owner = find(&windows::window_class(&temp.0));
    let rail = find(&rail_class(&temp.0));
    until(|| visible(rail));
    assert_eq!(rect(rail), moved);
    assert!(WidgetConfig::load(&temp.0).unwrap().locked);
    post(owner, WM_COMMAND, CMD_RESET as usize);
    until(|| WidgetConfig::load(&temp.0).unwrap().placement.is_none());
    assert_eq!(rect(rail), initial);
    post(owner, WM_COMMAND, CMD_LOCK as usize);
    until(|| !WidgetConfig::load(&temp.0).unwrap().locked);
    if monitor.rcMonitor.bottom > monitor.rcWork.bottom {
        let top = monitor.rcWork.bottom
            + (monitor.rcMonitor.bottom - monitor.rcWork.bottom - initial.height()) / 2;
        drag(rail, 0, top - rect(rail).top);
        let overlay = rect(rail);
        assert_eq!(overlay.top, top);
        let center = POINT {
            x: overlay.left + overlay.width() / 2,
            y: overlay.top + overlay.height() / 2,
        };
        assert_eq!(
            unsafe { WindowFromPoint(center) },
            rail,
            "the rail must actually cover the taskbar at its center"
        );
        // Regression: opening details over the taskbar must neither move the rail
        // nor prevent a subsequent drag back into the workspace.
        let card = find(&card_class(&temp.0));
        for _ in 0..5 {
            click(rail);
            until(|| visible(card));
            post(owner, WM_ENVIRONMENT, 0);
            std::thread::sleep(Duration::from_millis(60));
            assert_eq!(
                rect(rail),
                overlay,
                "opening details moved the taskbar rail"
            );
            send(card, WM_KEYDOWN, VK_ESCAPE as usize, 0);
            until(|| !visible(card));
            assert_eq!(
                rect(rail),
                overlay,
                "closing details moved the taskbar rail"
            );
        }
        real_drag(rail, -px(120.0, dpi), -px(140.0, dpi));
        assert!(
            rect(rail).bottom <= monitor.rcWork.bottom,
            "taskbar rail could not be dragged out"
        );
    }
    stop(&mut again, owner);
    assert!(find(&rail_class(&temp.0)).is_null());
    assert!(find(&card_class(&temp.0)).is_null());
}

#[test]
fn taskbar_details_do_not_move_or_bury_the_rail_and_real_drag_can_leave() {
    let _guard = UI_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _dpi =
        Dpi(
            unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) }
                as isize,
        );
    let temp = Temp::new();
    let mut app = launch(&temp.0);
    until(|| text(find(&windows::window_class(&temp.0))).contains("正常"));
    let owner = find(&windows::window_class(&temp.0));
    let rail = find(&rail_class(&temp.0));
    let card = find(&card_class(&temp.0));
    let dpi = unsafe { GetDpiForWindow(rail) };
    let mut monitor: MONITORINFO = unsafe { std::mem::zeroed() };
    monitor.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    assert_ne!(
        unsafe {
            GetMonitorInfoW(
                MonitorFromWindow(rail, MONITOR_DEFAULTTONEAREST),
                &mut monitor,
            )
        },
        0
    );
    let initial = rect(rail);
    let top = monitor.rcWork.bottom
        + (monitor.rcMonitor.bottom - monitor.rcWork.bottom - initial.height()) / 2;
    drag(
        rail,
        monitor.rcMonitor.left + px(300.0, dpi) - initial.left,
        top - initial.top,
    );
    let saved = rect(rail);
    real_drag(rail, 0, 0);
    until(|| visible(card));
    assert_eq!(
        unsafe {
            WindowFromPoint(POINT {
                x: saved.left + saved.width() / 2,
                y: saved.top + saved.height() / 2,
            })
        },
        rail,
        "opening details buried the taskbar rail"
    );
    // Exercise the separate full-details modal, not only the compact card.
    send(card, WM_KEYDOWN, VK_TAB as usize, 0);
    send(card, WM_KEYDOWN, VK_RETURN as usize, 0);
    let mut dialog = null_mut();
    until(|| {
        dialog = unsafe {
            FindWindowW(
                wide("#32770").as_ptr(),
                wide("QuotaRail · 额度详情").as_ptr(),
            )
        };
        !dialog.is_null()
    });
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(dialog, &mut pid) };
    assert_eq!(pid, app.0.id(), "only interact with our own test dialog");
    assert_eq!(
        unsafe { GetWindow(dialog, GW_OWNER) },
        rail,
        "full details must belong to the visible rail"
    );
    until(|| visible(dialog));
    post(dialog, WM_CLOSE, 0);
    until(|| unsafe { IsWindow(dialog) == 0 });
    post(owner, WM_ENVIRONMENT, 0);
    std::thread::sleep(Duration::from_millis(60));
    assert_eq!(rect(rail), saved, "details changed the saved rail position");
    real_drag(rail, 0, -px(140.0, dpi));
    assert!(
        rect(rail).bottom <= monitor.rcWork.bottom,
        "the rail could not leave the taskbar after details"
    );
    stop(&mut app, owner);
}
