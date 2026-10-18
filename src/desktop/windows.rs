//! Native shell integration. The window procedure only queues intents, keeping Rust
//! state borrows out of re-entrant Win32 menu/dialog dispatch.
use super::{
    DesktopOptions,
    autostart::{self, StartupEntry},
    settings::{self, SettingsWindow},
    state::{DesktopState, manual_allowed},
    view,
    widget::{self, Widget},
    worker::{Command, SharedState, Worker},
};
use crate::{
    error::{ErrorKind, ProbeError, Result},
    storage::{path_key, unix_now},
};
use std::{
    mem::{size_of, zeroed},
    ptr::{null, null_mut},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU32, Ordering},
    },
};
use windows_sys::{
    Win32::{
        Foundation::{
            CloseHandle, ERROR_ALREADY_EXISTS, FILETIME, GetLastError, HANDLE, HWND, LPARAM,
            LRESULT, POINT, SYSTEMTIME, WPARAM,
        },
        System::{
            LibraryLoader::GetModuleHandleW,
            Threading::CreateMutexW,
            Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime},
        },
        UI::{
            HiDpi::{
                DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow,
                GetSystemMetricsForDpi, SetProcessDpiAwarenessContext,
            },
            Shell::*,
            WindowsAndMessaging::*,
        },
    },
    core::GUID,
};

pub const WM_UPDATED: u32 = WM_APP + 41;
pub const WM_MENU: u32 = WM_APP + 42;
pub const WM_DETAILS: u32 = WM_APP + 43;
const WM_TRAY: u32 = WM_APP + 44;
const WM_PENDING: u32 = WM_APP + 45;
pub const CMD_REFRESH: u32 = 101;
pub const CMD_RECONNECT: u32 = 102;
pub const CMD_DETAILS: u32 = 103;
pub const CMD_HISTORY: u32 = 104;
pub const CMD_EXIT: u32 = 105;
const TIMER_STATUS: usize = 1;
static TASKBAR_MESSAGE: AtomicU32 = AtomicU32::new(0);
static PENDING_EVENTS: AtomicU32 = AtomicU32::new(0);
const READD_ICON: u32 = 1;
const WAKE_WORKER: u32 = 2;
const UPDATE_VIEW: u32 = 4;

pub fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

pub fn window_class(data_dir: &std::path::Path) -> String {
    format!("QuotaRail.Tray.{}", &path_key(data_dir)[..24])
}

pub fn show_error(error: &ProbeError) {
    let text = wide(&format!("QuotaRail 无法启动。\n{}", error.message));
    let title = wide("QuotaRail");
    // SAFETY: static reviewed diagnostics, valid terminated UTF-16 buffers.
    unsafe {
        MessageBoxW(
            null_mut(),
            text.as_ptr(),
            title.as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
}

struct Instance(HANDLE);
impl Drop for Instance {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

fn singleton(class: &str) -> Result<Option<Instance>> {
    // Local kernel-object namespace already isolates Terminal Services sessions.
    let name = wide(&format!("Local\\{class}"));
    let handle = unsafe { CreateMutexW(null(), 0, name.as_ptr()) };
    if handle.is_null() {
        return Err(platform_error());
    }
    let existed = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    let instance = Instance(handle);
    if existed {
        let class_name = wide(class);
        for _ in 0..10 {
            let window = unsafe { FindWindowW(class_name.as_ptr(), null()) };
            if !window.is_null() {
                unsafe {
                    PostMessageW(window, WM_DETAILS, 0, 0);
                }
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        return Ok(None);
    }
    Ok(Some(instance))
}

struct Window(HWND);
impl Drop for Window {
    fn drop(&mut self) {
        unsafe {
            DestroyWindow(self.0);
        }
    }
}
struct Icon(HICON);
impl Drop for Icon {
    fn drop(&mut self) {
        unsafe {
            DestroyIcon(self.0);
        }
    }
}

struct Tray {
    hwnd: HWND,
    icon: Icon,
    last_tip: String,
}
impl Tray {
    fn data(&self, tip: &str) -> NOTIFYICONDATAW {
        let mut data: NOTIFYICONDATAW = unsafe { zeroed() };
        data.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
        data.hWnd = self.hwnd;
        data.uID = 1;
        data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP;
        data.uCallbackMessage = WM_TRAY;
        data.hIcon = self.icon.0;
        let mut offset = 0;
        for ch in tip.chars() {
            let mut units = [0u16; 2];
            let encoded = ch.encode_utf16(&mut units);
            if offset + encoded.len() >= data.szTip.len() {
                break;
            }
            data.szTip[offset..offset + encoded.len()].copy_from_slice(encoded);
            offset += encoded.len();
        }
        data
    }

    fn add(&self, tip: &str) -> Result<()> {
        let mut data = self.data(tip);
        if unsafe { Shell_NotifyIconW(NIM_ADD, &data) } == 0 {
            return Err(platform_error());
        }
        data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
        if unsafe { Shell_NotifyIconW(NIM_SETVERSION, &data) } == 0 {
            return Err(platform_error());
        }
        Ok(())
    }

    fn update(&mut self, state: &DesktopState, readd: bool) -> Result<()> {
        let tip = view::tooltip(state, unix_now());
        if !readd && tip == self.last_tip {
            return Ok(());
        }
        let result = if readd {
            self.add(&tip)
        } else {
            let data = self.data(&tip);
            if unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) } == 0 {
                self.add(&tip)
            } else {
                Ok(())
            }
        };
        if result.is_ok() {
            self.last_tip = tip;
        }
        result
    }
}
impl Drop for Tray {
    fn drop(&mut self) {
        let data = self.data("");
        unsafe {
            Shell_NotifyIconW(NIM_DELETE, &data);
        }
    }
}

fn logo_icon(size: i32) -> Result<Icon> {
    // Trusted, compile-time asset. Keep its original pixel artwork; select an
    // authored DPI size instead of parsing SVG or rasterizing on every update.
    const LOGO: &[u8] = include_bytes!("../../assets/quotarail.ico");
    let count = u16::from_le_bytes([LOGO[4], LOGO[5]]) as usize;
    let entry = LOGO[6..6 + count * 16]
        .as_chunks::<16>()
        .0
        .iter()
        .min_by_key(|entry| (i32::from(entry[0]) - size).abs())
        .ok_or_else(platform_error)?;
    let length = u32::from_le_bytes(entry[8..12].try_into().unwrap()) as usize;
    let offset = u32::from_le_bytes(entry[12..16].try_into().unwrap()) as usize;
    let bytes = LOGO
        .get(offset..offset + length)
        .ok_or_else(platform_error)?;
    let icon = unsafe {
        CreateIconFromResourceEx(bytes.as_ptr(), length as u32, 1, 0x00030000, size, size, 0)
    };
    if icon.is_null() {
        Err(platform_error())
    } else {
        Ok(Icon(icon))
    }
}

fn make_icon(hwnd: HWND) -> Result<Icon> {
    let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96);
    let size = unsafe { GetSystemMetricsForDpi(SM_CXSMICON, dpi) }.clamp(16, 64);
    logo_icon(size)
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        value if value != 0 && value == TASKBAR_MESSAGE.load(Ordering::Relaxed) => {
            // A modal menu/dialog can dispatch this before the outer loop sees it.
            PENDING_EVENTS.fetch_or(READD_ICON, Ordering::Relaxed);
            unsafe {
                PostMessageW(hwnd, WM_PENDING, 0, 0);
            }
            0
        }
        WM_POWERBROADCAST if matches!(wparam, 7 | 18) => {
            PENDING_EVENTS.fetch_or(WAKE_WORKER, Ordering::Relaxed);
            unsafe {
                PostMessageW(hwnd, WM_PENDING, 0, 0);
            }
            1
        }
        WM_UPDATED | WM_TIMER => {
            PENDING_EVENTS.fetch_or(UPDATE_VIEW, Ordering::Relaxed);
            unsafe {
                PostMessageW(hwnd, WM_PENDING, 0, 0);
            }
            0
        }
        WM_TRAY => {
            let event = lparam as u32 & 0xffff;
            if event == WM_CONTEXTMENU || event == WM_RBUTTONUP {
                unsafe {
                    PostMessageW(hwnd, WM_MENU, 0, 0);
                }
            } else if event == NIN_SELECT || event == (NIN_SELECT | 1) {
                unsafe {
                    PostMessageW(hwnd, WM_DETAILS, 0, 0);
                }
            }
            0
        }
        WM_DESTROY => {
            unsafe {
                PostQuitMessage(0);
            }
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

fn show_menu(
    hwnd: HWND,
    state: &DesktopState,
    widget: Option<&Widget>,
    at_cursor: bool,
    settings_warning: bool,
) -> u32 {
    let menu = unsafe { CreatePopupMenu() };
    if menu.is_null() {
        return 0;
    }
    let append = |id: u32, text: &str, enabled: bool| {
        let value = wide(text);
        unsafe {
            AppendMenuW(
                menu,
                MF_STRING | if enabled { 0 } else { MF_GRAYED },
                id as usize,
                value.as_ptr(),
            );
        }
    };
    append(
        0,
        &format!("QuotaRail · {}", view::status(state, unix_now())),
        false,
    );
    if let Some(bucket) = state.snapshot.as_ref().and_then(view::primary_bucket) {
        append(0, &view::window_line(bucket, 300), false);
        append(0, &view::window_line(bucket, 10080), false);
    }
    unsafe {
        AppendMenuW(menu, MF_SEPARATOR, 0, null());
    }
    append(CMD_DETAILS, "查看额度详情", true);
    append(
        CMD_HISTORY,
        "查看历史缓存（未核验）",
        state.history.is_some(),
    );
    append(CMD_REFRESH, "刷新额度", manual_allowed(unix_now(), state));
    append(
        CMD_RECONNECT,
        "重新连接 Codex",
        manual_allowed(unix_now(), state),
    );
    if let Some(widget) = widget {
        let config = widget.config();
        if let Some(status) = widget.taskbar_status() {
            append(0, &format!("当前模式：任务栏内部 · {status}"), false);
            append(widget::CMD_REBIND, "重新挂载任务栏", true);
        } else {
            append(0, "当前模式：独立悬浮窗口", false);
        }
        unsafe {
            AppendMenuW(menu, MF_SEPARATOR, 0, null());
        }
        append(
            widget::CMD_VISIBLE,
            if config.visible {
                "隐藏常驻条"
            } else {
                "显示常驻条"
            },
            true,
        );
        append(
            widget::CMD_LOCK,
            if config.locked {
                "解锁位置"
            } else {
                "锁定位置"
            },
            true,
        );
        append(widget::CMD_RESET, "恢复默认位置", true);
        append(
            widget::CMD_THEME_SYSTEM,
            "主题：跟随系统",
            config.theme != super::widget_layout::Theme::System,
        );
        append(
            widget::CMD_THEME_LIGHT,
            "主题：浅色",
            config.theme != super::widget_layout::Theme::Light,
        );
        append(
            widget::CMD_THEME_DARK,
            "主题：深色",
            config.theme != super::widget_layout::Theme::Dark,
        );
    }
    unsafe {
        AppendMenuW(menu, MF_SEPARATOR, 0, null());
    }
    if settings_warning {
        append(0, "设置文件无法读取；打开设置可恢复", false);
    }
    append(settings::CMD_SETTINGS, "设置…", true);
    append(CMD_EXIT, "退出 QuotaRail", true);
    let mut point = POINT { x: 0, y: 0 };
    let identifier = NOTIFYICONIDENTIFIER {
        cbSize: size_of::<NOTIFYICONIDENTIFIER>() as u32,
        hWnd: hwnd,
        uID: 1,
        guidItem: GUID::from_u128(0),
    };
    let mut rectangle = unsafe { zeroed() };
    if !at_cursor && unsafe { Shell_NotifyIconGetRect(&identifier, &mut rectangle) } == 0 {
        point.x = rectangle.left;
        point.y = rectangle.top;
    } else {
        unsafe {
            GetCursorPos(&mut point);
        }
    }
    unsafe {
        SetForegroundWindow(hwnd);
    }
    let command = unsafe {
        TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON,
            point.x,
            point.y,
            0,
            hwnd,
            null(),
        )
    } as u32;
    unsafe {
        DestroyMenu(menu);
        PostMessageW(hwnd, WM_NULL, 0, 0);
    }
    command
}

fn show_details(hwnd: HWND, state: &DesktopState, historical: bool, widget: Option<&Widget>) {
    // Dismiss the compact card before entering the system modal loop. The visible
    // rail owns this dialog, preserving its topmost group above the taskbar.
    let dialog_owner = widget.map_or(hwnd, Widget::dialog_owner);
    let text = if historical {
        view::historical_details(state, unix_now(), local_time)
    } else {
        view::details(state, unix_now(), local_time)
    };
    let text = wide(&text);
    let title = wide(if historical {
        "QuotaRail · 历史缓存"
    } else {
        "QuotaRail · 额度详情"
    });
    unsafe {
        MessageBoxW(
            dialog_owner,
            text.as_ptr(),
            title.as_ptr(),
            MB_OK | MB_ICONINFORMATION,
        );
    }
    if let Some(widget) = widget {
        widget.restore_after_dialog();
    }
    // Repeated clicks during the modal dialog need not create a queue of duplicate dialogs.
    let mut message = unsafe { zeroed() };
    unsafe { while PeekMessageW(&mut message, hwnd, WM_DETAILS, WM_DETAILS, PM_REMOVE) != 0 {} }
}

pub fn local_time(seconds: i64) -> String {
    let Some(ticks) = u64::try_from(seconds)
        .ok()
        .and_then(|s| s.checked_mul(10_000_000))
        .and_then(|s| s.checked_add(116_444_736_000_000_000))
    else {
        return "未知".into();
    };
    let file = FILETIME {
        dwLowDateTime: ticks as u32,
        dwHighDateTime: (ticks >> 32) as u32,
    };
    let mut utc: SYSTEMTIME = unsafe { zeroed() };
    let mut local: SYSTEMTIME = unsafe { zeroed() };
    if unsafe { FileTimeToSystemTime(&file, &mut utc) } == 0
        || unsafe { SystemTimeToTzSpecificLocalTime(null(), &utc, &mut local) } == 0
    {
        return "未知".into();
    }
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        local.wYear, local.wMonth, local.wDay, local.wHour, local.wMinute, local.wSecond
    )
}

pub fn local_zone() -> String {
    use windows_sys::Win32::System::Time::{GetTimeZoneInformation, TIME_ZONE_INFORMATION};
    let mut zone: TIME_ZONE_INFORMATION = unsafe { zeroed() };
    let kind = unsafe { GetTimeZoneInformation(&mut zone) };
    if kind == u32::MAX {
        return "本地时区".into();
    }
    let bias = i64::from(zone.Bias)
        + i64::from(if kind == 2 {
            zone.DaylightBias
        } else {
            zone.StandardBias
        });
    let offset = -bias;
    format!(
        "UTC{}{:02}:{:02}",
        if offset < 0 { '-' } else { '+' },
        offset.abs() / 60,
        offset.abs() % 60
    )
}

pub fn run(mut options: DesktopOptions) -> Result<()> {
    std::fs::create_dir_all(&options.data_dir).map_err(|_| platform_error())?;
    let class = window_class(&options.data_dir);
    let Some(_instance) = singleton(&class)? else {
        return Ok(());
    };
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    let class_name = wide(&class);
    let title = wide("QuotaRail · 启动中");
    let instance = unsafe { GetModuleHandleW(null()) };
    let window_class = WNDCLASSW {
        lpfnWndProc: Some(window_proc),
        hInstance: instance,
        lpszClassName: class_name.as_ptr(),
        ..unsafe { zeroed() }
    };
    if unsafe { RegisterClassW(&window_class) } == 0 {
        return Err(platform_error());
    }
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_TOOLWINDOW,
            class_name.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            null_mut(),
            null_mut(),
            instance,
            null(),
        )
    };
    if hwnd.is_null() {
        return Err(platform_error());
    }
    let _window = Window(hwnd);
    let shared: SharedState = Arc::new(Mutex::new(DesktopState {
        refresh_minutes: options.refresh_minutes,
        ..Default::default()
    }));
    let mut tray = Tray {
        hwnd,
        icon: make_icon(hwnd)?,
        last_tip: String::new(),
    };
    tray.add("QuotaRail · 正在读取 Codex 额度")?;
    let mut widget = if options.tray_only {
        None
    } else {
        Widget::new(
            hwnd,
            shared.clone(),
            &options.data_dir,
            options.taskbar.then_some(options.taskbar_left_space),
        )
        .ok()
    };
    let hwnd_bits = hwnd as usize;
    let mut worker = Worker::start(options.clone(), shared.clone(), move || unsafe {
        PostMessageW(hwnd_bits as HWND, WM_UPDATED, 0, 0);
    })?;
    let taskbar_created = unsafe { RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()) };
    TASKBAR_MESSAGE.store(taskbar_created, Ordering::Relaxed);
    unsafe {
        SetTimer(hwnd, TIMER_STATUS, 30_000, None);
    }
    let mut message: MSG = unsafe { zeroed() };
    let mut result = Ok(());
    let mut settings_window: Option<SettingsWindow> = None;
    loop {
        let read = unsafe { GetMessageW(&mut message, null_mut(), 0, 0) };
        if read == 0 {
            break;
        }
        if read == -1 {
            result = Err(platform_error());
            break;
        }
        if message.hwnd == hwnd {
            let state = shared.lock().unwrap_or_else(|e| e.into_inner()).clone();
            let mut action = 0;
            match message.message {
                WM_CLOSE => break,
                WM_UPDATED | WM_TIMER => {
                    let caption =
                        wide(&format!("QuotaRail · {}", view::status(&state, unix_now())));
                    unsafe {
                        SetWindowTextW(hwnd, caption.as_ptr());
                    }
                    // During Explorer startup we retain the data and retry on its next broadcast/timer.
                    let _ = tray.update(&state, false);
                    if let Some(widget) = &widget {
                        if widget.taskbar_status().is_some() {
                            widget.environment();
                        } else {
                            widget.update();
                        }
                    }
                    continue;
                }
                WM_MENU => {
                    action = show_menu(
                        hwnd,
                        &state,
                        widget.as_ref(),
                        message.wParam == 1,
                        options.settings_warning,
                    )
                }
                settings::WM_SETTINGS_APPLY => action = 107,
                WM_DETAILS => action = CMD_DETAILS,
                WM_COMMAND => action = (message.wParam & 0xffff) as u32,
                WM_POWERBROADCAST if matches!(message.wParam, 7 | 18) => {
                    let _ = worker.sender.try_send(Command::Wake);
                    continue;
                }
                value if taskbar_created != 0 && value == taskbar_created => {
                    let _ = tray.update(&state, true);
                    if let Some(widget) = &widget {
                        widget.environment();
                    }
                    continue;
                }
                widget::WM_ENVIRONMENT | WM_DISPLAYCHANGE | WM_SETTINGCHANGE | WM_THEMECHANGED => {
                    if let Some(widget) = &widget {
                        widget.environment();
                    }
                    continue;
                }
                _ => {}
            }
            match action {
                CMD_REFRESH if manual_allowed(unix_now(), &state) => {
                    let _ = worker.sender.try_send(Command::Refresh);
                }
                CMD_RECONNECT if manual_allowed(unix_now(), &state) => {
                    let _ = worker.sender.try_send(Command::Reconnect);
                }
                CMD_DETAILS => {
                    if message.lParam == 1 {
                        show_details(hwnd, &state, false, widget.as_ref());
                    } else if let Some(widget) = &widget {
                        widget.show_details();
                    } else {
                        show_details(hwnd, &state, false, None);
                    }
                }
                CMD_HISTORY => show_details(hwnd, &state, true, widget.as_ref()),
                settings::CMD_SETTINGS => {
                    if let Some(window) = &settings_window
                        && unsafe { IsWindowVisible(window.hwnd) } != 0
                    {
                        window.show();
                        continue;
                    }
                    if let Some(widget) = &widget {
                        widget.dialog_owner();
                    }
                    let config = widget.as_ref().map(Widget::config).unwrap_or_else(|| {
                        super::widget_layout::WidgetConfig::load(&options.data_dir)
                            .unwrap_or_default()
                    });
                    settings_window = None;
                    match SettingsWindow::new(hwnd, &options, &config) {
                        Ok(window) => {
                            window.show();
                            settings_window = Some(window);
                        }
                        Err(_) => show_error(&ProbeError::new(
                            ErrorKind::Io,
                            "设置窗口无法创建；额度监控继续运行。",
                        )),
                    }
                }
                107 => {
                    if let Some(window) = &settings_window
                        && let Some(change) = window.take()
                    {
                        let next = match options.updated(&change.preferences) {
                            Ok(next) => next,
                            Err(_) => {
                                window.feedback("设置无效，请检查路径与数值。");
                                continue;
                            }
                        };
                        if change.preferences.save(&options.data_dir).is_err() {
                            window.feedback("设置文件未能保存；本次变更尚未应用。");
                            continue;
                        }
                        let mut config = widget.as_ref().map(Widget::config).unwrap_or_else(|| {
                            super::widget_layout::WidgetConfig::load(&options.data_dir)
                                .unwrap_or_default()
                        });
                        config.theme = change.theme;
                        config.visible = change.visible;
                        config.locked = change.locked;
                        if change.reset_position {
                            config.placement = None;
                            config.taskbar_offset_dip = 0.0;
                        }
                        let appearance = if let Some(widget) = &widget {
                            widget.apply_config(config)
                        } else {
                            config.save(&options.data_dir)
                        };
                        if appearance.is_err() {
                            window.feedback(
                                "应用设置已保存，但外观设置无法保存；请检查数据目录后再次保存。",
                            );
                            continue;
                        }
                        let startup_result: std::io::Result<()> = (|| {
                            let entry = StartupEntry::default();
                            if let Some(enabled) = change.startup {
                                if enabled {
                                    let command = autostart::command(
                                        &std::env::current_exe()?,
                                        &options.data_dir,
                                    )?;
                                    if entry.read()?.as_deref() != Some(command.as_str()) {
                                        entry.write(Some(&command))?;
                                    }
                                } else {
                                    entry.write(None)?;
                                }
                            }
                            Ok(())
                        })();
                        let source_changed = options.codex != next.codex
                            || options.home != next.home
                            || !worker.is_running();
                        if source_changed {
                            worker.shutdown();
                            {
                                let mut state = shared.lock().unwrap_or_else(|e| e.into_inner());
                                let generation = state.generation.wrapping_add(1);
                                *state = DesktopState {
                                    generation,
                                    refresh_minutes: next.refresh_minutes,
                                    loading: true,
                                    ..Default::default()
                                };
                            }
                            match Worker::start(next.clone(), shared.clone(), move || unsafe {
                                PostMessageW(hwnd_bits as HWND, WM_UPDATED, 0, 0);
                            }) {
                                Ok(replacement) => worker = replacement,
                                Err(error) => {
                                    let mut state =
                                        shared.lock().unwrap_or_else(|e| e.into_inner());
                                    state.loading = false;
                                    state.error = Some(error);
                                }
                            }
                        } else if options.refresh_minutes != next.refresh_minutes {
                            shared
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .refresh_minutes = next.refresh_minutes;
                            worker.set_refresh_minutes(next.refresh_minutes);
                        }
                        let rebuild = options.taskbar != next.taskbar
                            || options.tray_only != next.tray_only
                            || (next.taskbar
                                && options.taskbar_left_space != next.taskbar_left_space)
                            || (!next.tray_only && widget.is_none());
                        if rebuild {
                            drop(widget.take());
                            widget = if next.tray_only {
                                None
                            } else {
                                Widget::new(
                                    hwnd,
                                    shared.clone(),
                                    &next.data_dir,
                                    next.taskbar.then_some(next.taskbar_left_space),
                                )
                                .ok()
                            };
                        }
                        options = next;
                        if let Some(widget) = &widget {
                            widget.update();
                        }
                        unsafe {
                            PostMessageW(hwnd, WM_UPDATED, 0, 0);
                        }
                        window.hide();
                        if startup_result.is_err() {
                            window.feedback(
                                "其他设置已保存并生效，但开机启动未能更新；请再次打开设置重试。",
                            );
                        }
                        if !options.tray_only && widget.is_none() {
                            window.feedback(
                                "设置已保存，但额度窗口创建失败；可从托盘重新打开设置恢复。",
                            );
                        }
                    }
                }
                CMD_EXIT => break,
                _ => {
                    if let Some(widget) = &widget {
                        widget.command(action);
                    }
                }
            }
        }
        let pending = PENDING_EVENTS.swap(0, Ordering::Relaxed);
        if pending & WAKE_WORKER != 0 {
            let _ = worker.sender.try_send(Command::Wake);
        }
        if pending & (READD_ICON | UPDATE_VIEW) != 0 {
            let state = shared.lock().unwrap_or_else(|e| e.into_inner()).clone();
            let caption = wide(&format!("QuotaRail · {}", view::status(&state, unix_now())));
            unsafe {
                SetWindowTextW(hwnd, caption.as_ptr());
            }
            let _ = tray.update(&state, pending & READD_ICON != 0);
            if let Some(widget) = &widget {
                widget.update();
            }
        }
        if let Some(window) = &settings_window
            && unsafe { IsWindowVisible(window.hwnd) } != 0
            && unsafe { IsDialogMessageW(window.hwnd, &message) } != 0
        {
            continue;
        }
        unsafe {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    worker.shutdown();
    unsafe {
        KillTimer(hwnd, TIMER_STATUS);
    }
    result
}

fn platform_error() -> ProbeError {
    ProbeError::new(ErrorKind::Io, "The native Windows tray is unavailable.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::Graphics::Gdi::{BITMAP, DeleteObject, GetObjectW};

    #[test]
    fn supplied_logo_decodes_at_all_tray_dpi_sizes() {
        for size in [16, 20, 24, 32, 40, 48, 64] {
            let icon = logo_icon(size).unwrap();
            let mut info: ICONINFO = unsafe { zeroed() };
            assert_ne!(unsafe { GetIconInfo(icon.0, &mut info) }, 0);
            let mut bitmap: BITMAP = unsafe { zeroed() };
            let read = unsafe {
                GetObjectW(
                    info.hbmColor,
                    size_of::<BITMAP>() as i32,
                    (&mut bitmap as *mut BITMAP).cast(),
                )
            };
            unsafe {
                DeleteObject(info.hbmColor);
                DeleteObject(info.hbmMask);
            }
            assert_ne!(read, 0);
            assert_eq!(
                (bitmap.bmWidth, bitmap.bmHeight, bitmap.bmBitsPixel),
                (size, size, 32)
            );
        }
    }
}
