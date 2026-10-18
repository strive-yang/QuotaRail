//! Modeless native settings, owned by QuotaRail rather than Explorer.
use super::{
    DesktopOptions,
    autostart::StartupEntry,
    preferences::{DisplayMode, Preferences},
    widget_layout::{Theme, WidgetConfig, px},
    windows::wide,
};
use std::{
    cell::{Cell, RefCell},
    mem::{size_of, zeroed},
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        Controls::EM_LIMITTEXT, HiDpi::*, Input::KeyboardAndMouse::EnableWindow,
        WindowsAndMessaging::*,
    },
};

pub const CMD_SETTINGS: u32 = 106;
pub const WM_SETTINGS_APPLY: u32 = WM_APP + 70;
pub const ID_THEME: i32 = 201;
pub const ID_MODE: i32 = 202;
pub const ID_VISIBLE: i32 = 203;
pub const ID_LOCKED: i32 = 204;
pub const ID_RESET_POSITION: i32 = 205;
pub const ID_REFRESH: i32 = 206;
pub const ID_LEFT_SPACE: i32 = 207;
pub const ID_CODEX: i32 = 208;
pub const ID_HOME: i32 = 209;
pub const ID_STARTUP: i32 = 210;
const ID_DEFAULTS: i32 = 211;
pub struct Submission {
    pub preferences: Preferences,
    pub theme: Theme,
    pub visible: bool,
    pub locked: bool,
    pub reset_position: bool,
    pub startup: Option<bool>,
}
struct Data {
    owner: HWND,
    dpi: Cell<u32>,
    font: Cell<HFONT>,
    controls: RefCell<Vec<(HWND, [i32; 4])>>,
    reset: Cell<bool>,
    startup_initial: bool,
    startup_readable: bool,
    submission: RefCell<Option<Submission>>,
}
pub(crate) struct SettingsWindow {
    pub hwnd: HWND,
    data: Box<Data>,
}
impl Drop for SettingsWindow {
    fn drop(&mut self) {
        unsafe {
            DestroyWindow(self.hwnd);
            DeleteObject(self.data.font.get());
        }
    }
}
pub fn class(root: &std::path::Path) -> String {
    format!("{}.Settings", super::windows::window_class(root))
}
fn make_font(dpi: u32) -> HFONT {
    unsafe {
        CreateFontW(
            -px(12.0, dpi),
            0,
            0,
            0,
            400,
            0,
            0,
            0,
            DEFAULT_CHARSET as u32,
            0,
            0,
            CLEARTYPE_QUALITY as u32,
            0,
            wide("Segoe UI").as_ptr(),
        )
    }
}
impl SettingsWindow {
    pub(crate) fn new(
        owner: HWND,
        options: &DesktopOptions,
        widget: &WidgetConfig,
    ) -> std::io::Result<Self> {
        let dpi = unsafe { GetDpiForWindow(owner) }.max(96);
        let font = make_font(dpi);
        if font.is_null() {
            return Err(std::io::Error::last_os_error());
        }
        let startup = StartupEntry::default().read();
        let mut data = Box::new(Data {
            owner,
            dpi: Cell::new(dpi),
            font: Cell::new(font),
            controls: RefCell::new(Vec::new()),
            reset: Cell::new(false),
            startup_initial: startup.as_ref().is_ok_and(|v| v.is_some()),
            startup_readable: startup.is_ok(),
            submission: RefCell::new(None),
        });
        let name = wide(&class(&options.data_dir));
        let wc = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: unsafe { GetModuleHandleW(null()) },
            hCursor: unsafe { LoadCursorW(null_mut(), IDC_ARROW) },
            hbrBackground: (COLOR_WINDOW + 1) as HBRUSH,
            lpszClassName: name.as_ptr(),
            ..unsafe { zeroed() }
        };
        if unsafe { RegisterClassW(&wc) } == 0
            && unsafe { GetLastError() } != ERROR_CLASS_ALREADY_EXISTS
        {
            unsafe {
                DeleteObject(font);
            }
            return Err(std::io::Error::last_os_error());
        }
        let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU;
        let ex = WS_EX_TOOLWINDOW | WS_EX_CONTROLPARENT | WS_EX_DLGMODALFRAME;
        let mut r = RECT {
            left: 0,
            top: 0,
            right: px(520.0, dpi),
            bottom: px(508.0, dpi),
        };
        unsafe {
            AdjustWindowRectExForDpi(&mut r, style, 0, ex, dpi);
        }
        let mut monitor: MONITORINFO = unsafe { zeroed() };
        monitor.cbSize = size_of::<MONITORINFO>() as u32;
        unsafe {
            GetMonitorInfoW(
                MonitorFromWindow(owner, MONITOR_DEFAULTTOPRIMARY),
                &mut monitor,
            );
        }
        let hwnd = unsafe {
            CreateWindowExW(
                ex,
                name.as_ptr(),
                wide("QuotaRail 设置").as_ptr(),
                style,
                monitor.rcWork.left
                    + (monitor.rcWork.right - monitor.rcWork.left - (r.right - r.left)) / 2,
                monitor.rcWork.top
                    + (monitor.rcWork.bottom - monitor.rcWork.top - (r.bottom - r.top)) / 2,
                r.right - r.left,
                r.bottom - r.top,
                owner,
                null_mut(),
                wc.hInstance,
                (&mut *data as *mut Data).cast(),
            )
        };
        if hwnd.is_null() {
            unsafe {
                DeleteObject(font);
            }
            return Err(std::io::Error::last_os_error());
        }
        let result = Self { hwnd, data };
        result.build(options.settings_warning)?;
        result.populate(&options.preferences(), widget);
        check(hwnd, ID_STARTUP, result.data.startup_initial);
        if !result.data.startup_readable {
            unsafe {
                EnableWindow(GetDlgItem(hwnd, ID_STARTUP), 0);
            }
        }
        Ok(result)
    }
    fn control(
        &self,
        kind: &str,
        text: &str,
        id: i32,
        r: [i32; 4],
        style: u32,
    ) -> std::io::Result<()> {
        let dpi = self.data.dpi.get();
        let h = unsafe {
            CreateWindowExW(
                if kind == "EDIT" { WS_EX_CLIENTEDGE } else { 0 },
                wide(kind).as_ptr(),
                wide(text).as_ptr(),
                WS_CHILD | WS_VISIBLE | style,
                px(f64::from(r[0]), dpi),
                px(f64::from(r[1]), dpi),
                px(f64::from(r[2]), dpi),
                px(f64::from(r[3]), dpi),
                self.hwnd,
                id as HMENU,
                GetModuleHandleW(null()),
                null(),
            )
        };
        if h.is_null() {
            return Err(std::io::Error::last_os_error());
        }
        unsafe {
            SendMessageW(h, WM_SETFONT, self.data.font.get() as usize, 1);
        }
        self.data.controls.borrow_mut().push((h, r));
        Ok(())
    }
    fn build(&self, warning: bool) -> std::io::Result<()> {
        let label = |text, y| self.control("STATIC", text, 0, [20, y, 120, 22], 0);
        label("常驻显示", 16)?;
        label("额度显示主题", 48)?;
        label("显示方式", 86)?;
        for (id, y, items) in [
            (ID_THEME, 44, ["跟随系统", "浅色", "深色"]),
            (ID_MODE, 82, ["任务栏内部", "独立悬浮窗口", "仅系统托盘"]),
        ] {
            self.control(
                "COMBOBOX",
                "",
                id,
                [145, y, 350, 120],
                CBS_DROPDOWNLIST as u32 | WS_VSCROLL | WS_TABSTOP,
            )?;
            for text in items {
                unsafe {
                    SendMessageW(
                        GetDlgItem(self.hwnd, id),
                        CB_ADDSTRING,
                        0,
                        wide(text).as_ptr() as isize,
                    );
                }
            }
        }
        self.control(
            "BUTTON",
            "显示额度条",
            ID_VISIBLE,
            [20, 122, 140, 25],
            BS_AUTOCHECKBOX as u32 | WS_TABSTOP,
        )?;
        self.control(
            "BUTTON",
            "锁定位置",
            ID_LOCKED,
            [175, 122, 120, 25],
            BS_AUTOCHECKBOX as u32 | WS_TABSTOP,
        )?;
        self.control(
            "BUTTON",
            "恢复默认位置",
            ID_RESET_POSITION,
            [345, 120, 150, 29],
            WS_TABSTOP,
        )?;
        label("数据与刷新", 165)?;
        for (title, id, y, hint) in [
            ("刷新间隔（分钟）", ID_REFRESH, 198, "5–60；默认5"),
            (
                "天气预留（DIP）",
                ID_LEFT_SPACE,
                236,
                "0–1024；仅任务栏模式",
            ),
        ] {
            label(title, y)?;
            self.control(
                "EDIT",
                "",
                id,
                [145, y - 4, 82, 27],
                ES_NUMBER as u32 | WS_TABSTOP,
            )?;
            self.control("STATIC", hint, 0, [240, y, 258, 24], 0)?;
        }
        label("Codex CLI路径", 276)?;
        label("Codex登录目录", 314)?;
        for (id, y) in [(ID_CODEX, 272), (ID_HOME, 310)] {
            self.control(
                "EDIT",
                "",
                id,
                [145, y, 350, 27],
                ES_AUTOHSCROLL as u32 | WS_TABSTOP,
            )?;
            unsafe {
                SendMessageW(GetDlgItem(self.hwnd, id), EM_LIMITTEXT, 4096, 0);
            }
        }
        self.control(
            "STATIC",
            "路径留空时自动发现CLI，并沿用Codex默认登录目录。",
            0,
            [20, 348, 480, 22],
            0,
        )?;
        self.control(
            "BUTTON",
            if self.data.startup_readable {
                "登录Windows后自动运行（当前用户）"
            } else {
                "无法读取开机启动状态"
            },
            ID_STARTUP,
            [20, 378, 480, 26],
            BS_AUTOCHECKBOX as u32 | WS_TABSTOP,
        )?;
        self.control(
            "STATIC",
            if warning {
                "设置文件无法读取，当前使用默认值；保存会替换设置文件。"
            } else {
                "外观立即生效；更换Codex来源时会关闭旧会话并重新读取。"
            },
            0,
            [20, 416, 480, 30],
            0,
        )?;
        self.control(
            "BUTTON",
            "恢复默认",
            ID_DEFAULTS,
            [20, 463, 105, 30],
            WS_TABSTOP,
        )?;
        self.control(
            "BUTTON",
            "保存",
            1,
            [310, 463, 88, 30],
            BS_DEFPUSHBUTTON as u32 | WS_TABSTOP,
        )?;
        self.control("BUTTON", "取消", 2, [410, 463, 88, 30], WS_TABSTOP)
    }
    fn populate(&self, p: &Preferences, w: &WidgetConfig) {
        unsafe {
            SendMessageW(
                GetDlgItem(self.hwnd, ID_MODE),
                CB_SETCURSEL,
                match p.mode {
                    DisplayMode::Taskbar => 0,
                    DisplayMode::Floating => 1,
                    DisplayMode::Tray => 2,
                },
                0,
            );
            SendMessageW(
                GetDlgItem(self.hwnd, ID_THEME),
                CB_SETCURSEL,
                match w.theme {
                    Theme::System => 0,
                    Theme::Light => 1,
                    Theme::Dark => 2,
                },
                0,
            );
        }
        check(self.hwnd, ID_VISIBLE, w.visible);
        check(self.hwnd, ID_LOCKED, w.locked);
        for (id, value) in [
            (ID_REFRESH, p.refresh_minutes.to_string()),
            (ID_LEFT_SPACE, p.taskbar_left_space.to_string()),
            (
                ID_CODEX,
                p.codex
                    .as_ref()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            ),
            (
                ID_HOME,
                p.codex_home
                    .as_ref()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            ),
        ] {
            unsafe {
                SetWindowTextW(GetDlgItem(self.hwnd, id), wide(&value).as_ptr());
            }
        }
    }
    pub fn show(&self) {
        unsafe {
            ShowWindow(self.hwnd, SW_SHOWNORMAL);
            SetForegroundWindow(self.hwnd);
        }
    }
    pub fn hide(&self) {
        unsafe {
            ShowWindow(self.hwnd, SW_HIDE);
        }
    }
    pub fn take(&self) -> Option<Submission> {
        self.data.submission.borrow_mut().take()
    }
    pub fn feedback(&self, text: &str) {
        feedback(self.hwnd, text);
    }
}
fn check(h: HWND, id: i32, checked: bool) {
    unsafe {
        SendMessageW(GetDlgItem(h, id), BM_SETCHECK, usize::from(checked), 0);
    }
}
fn checked(h: HWND, id: i32) -> bool {
    unsafe { SendMessageW(GetDlgItem(h, id), BM_GETCHECK, 0, 0) == 1 }
}
fn text(h: HWND, id: i32) -> String {
    let mut value = [0u16; 4097];
    let n = unsafe { GetWindowTextW(GetDlgItem(h, id), value.as_mut_ptr(), value.len() as i32) };
    String::from_utf16_lossy(&value[..n.max(0) as usize])
        .trim()
        .to_owned()
}
fn feedback(h: HWND, text: &str) {
    unsafe {
        MessageBoxW(
            h,
            wide(text).as_ptr(),
            wide("QuotaRail · 设置提示").as_ptr(),
            MB_OK | MB_ICONINFORMATION,
        );
    }
}
fn read(h: HWND, data: &Data) -> std::result::Result<Submission, &'static str> {
    let p = Preferences {
        mode: match unsafe { SendMessageW(GetDlgItem(h, ID_MODE), CB_GETCURSEL, 0, 0) } {
            0 => DisplayMode::Taskbar,
            1 => DisplayMode::Floating,
            2 => DisplayMode::Tray,
            _ => return Err("请选择显示方式。"),
        },
        refresh_minutes: text(h, ID_REFRESH)
            .parse()
            .map_err(|_| "刷新间隔需为5至60的整数。")?,
        taskbar_left_space: text(h, ID_LEFT_SPACE)
            .parse()
            .map_err(|_| "天气预留需为0至1024的整数。")?,
        codex: Some(text(h, ID_CODEX))
            .filter(|v| !v.is_empty())
            .map(Into::into),
        codex_home: Some(text(h, ID_HOME))
            .filter(|v| !v.is_empty())
            .map(Into::into),
        ..Default::default()
    };
    p.validate()
        .map_err(|_| "请检查间隔、预留范围及路径；路径必须为绝对路径，不含引号。")?;
    if p.codex
        .as_ref()
        .is_some_and(|path| crate::discovery::find_codex(Some(path), None).is_err())
    {
        return Err("未找到可用的Codex原生exe或受支持的npm入口。");
    }
    if p.codex_home.as_ref().is_some_and(|path| !path.is_dir()) {
        return Err("Codex登录目录不存在，请选择已有目录或留空。");
    }
    let startup = checked(h, ID_STARTUP);
    Ok(Submission {
        preferences: p,
        theme: match unsafe { SendMessageW(GetDlgItem(h, ID_THEME), CB_GETCURSEL, 0, 0) } {
            0 => Theme::System,
            1 => Theme::Light,
            2 => Theme::Dark,
            _ => return Err("请选择主题。"),
        },
        visible: checked(h, ID_VISIBLE),
        locked: checked(h, ID_LOCKED),
        reset_position: data.reset.get(),
        startup: (data.startup_readable && (startup || startup != data.startup_initial))
            .then_some(startup),
    })
}
unsafe extern "system" fn window_proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if m == WM_NCCREATE {
        unsafe {
            let c = &*(l as *const CREATESTRUCTW);
            SetWindowLongPtrW(h, GWLP_USERDATA, c.lpCreateParams as isize);
        }
    }
    let ptr = unsafe { GetWindowLongPtrW(h, GWLP_USERDATA) } as *const Data;
    if ptr.is_null() {
        return unsafe { DefWindowProcW(h, m, w, l) };
    }
    let data = unsafe { &*ptr };
    match m {
        WM_CTLCOLORSTATIC | WM_CTLCOLORBTN => unsafe {
            SetBkMode(w as HDC, TRANSPARENT as i32);
            SetTextColor(w as HDC, GetSysColor(COLOR_WINDOWTEXT));
            GetSysColorBrush(COLOR_WINDOW) as LRESULT
        },
        WM_CLOSE => {
            unsafe {
                ShowWindow(h, SW_HIDE);
            }
            0
        }
        WM_COMMAND => {
            match (w & 0xffff) as i32 {
                1 => match read(h, data) {
                    Ok(value) => {
                        *data.submission.borrow_mut() = Some(value);
                        unsafe {
                            PostMessageW(data.owner, WM_SETTINGS_APPLY, 0, 0);
                        }
                    }
                    Err(e) => feedback(h, e),
                },
                2 => unsafe {
                    ShowWindow(h, SW_HIDE);
                },
                ID_RESET_POSITION => {
                    data.reset.set(true);
                    unsafe {
                        SetWindowTextW(
                            GetDlgItem(h, ID_RESET_POSITION),
                            wide("保存后恢复位置").as_ptr(),
                        );
                    }
                }
                ID_DEFAULTS => {
                    // Fill controls only; cancel never changes preferences or the registry.
                    for (id, index) in [(ID_MODE, 0), (ID_THEME, 0)] {
                        unsafe {
                            SendMessageW(GetDlgItem(h, id), CB_SETCURSEL, index, 0);
                        }
                    }
                    for (id, value) in [
                        (ID_REFRESH, "5"),
                        (ID_LEFT_SPACE, "240"),
                        (ID_CODEX, ""),
                        (ID_HOME, ""),
                    ] {
                        unsafe {
                            SetWindowTextW(GetDlgItem(h, id), wide(value).as_ptr());
                        }
                    }
                    check(h, ID_VISIBLE, true);
                    check(h, ID_LOCKED, false);
                    check(h, ID_STARTUP, false);
                    data.reset.set(true);
                }
                _ => {}
            }
            0
        }
        WM_DPICHANGED => {
            let dpi = (w & 0xffff) as u32;
            data.dpi.set(dpi);
            let font = make_font(dpi);
            let old = if !font.is_null() {
                Some(data.font.replace(font))
            } else {
                None
            };
            for (child, r) in data.controls.borrow().iter() {
                unsafe {
                    SetWindowPos(
                        *child,
                        null_mut(),
                        px(f64::from(r[0]), dpi),
                        px(f64::from(r[1]), dpi),
                        px(f64::from(r[2]), dpi),
                        px(f64::from(r[3]), dpi),
                        SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                    if old.is_some() {
                        SendMessageW(*child, WM_SETFONT, font as usize, 1);
                    }
                }
            }
            unsafe {
                if let Some(old) = old {
                    DeleteObject(old);
                }
                let r = &*(l as *const RECT);
                SetWindowPos(
                    h,
                    null_mut(),
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
            0
        }
        WM_NCDESTROY => unsafe {
            SetWindowLongPtrW(h, GWLP_USERDATA, 0);
            DefWindowProcW(h, m, w, l)
        },
        _ => unsafe { DefWindowProcW(h, m, w, l) },
    }
}
