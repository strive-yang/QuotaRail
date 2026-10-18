//! Owned Win32 windows with a taskbar child host and floating fallback. No credential access.
use super::{
    taskbar::{self, Host},
    widget_layout::*,
    widget_paint::{Canvas, Frame, Palette},
    windows::{self, wide},
    worker::SharedState,
};
use crate::{
    error::{ErrorKind, ProbeError, Result},
    storage::unix_now,
};
use ::windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
use std::{
    cell::{Cell, RefCell},
    mem::{size_of, zeroed},
    path::{Path, PathBuf},
    ptr::{null, null_mut},
    rc::Rc,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::{Dwm::*, Gdi::*},
    System::{LibraryLoader::GetModuleHandleW, Registry::*},
    UI::{Accessibility::*, HiDpi::*, Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
};

pub const WM_ENVIRONMENT: u32 = WM_APP + 60;
pub const CMD_VISIBLE: u32 = 110;
pub const CMD_LOCK: u32 = 111;
pub const CMD_RESET: u32 = 112;
pub const CMD_THEME_SYSTEM: u32 = 113;
pub const CMD_THEME_LIGHT: u32 = 114;
pub const CMD_THEME_DARK: u32 = 115;
pub const CMD_REBIND: u32 = 116;
const CARD_TIMER: usize = 2;
// WM_MOUSELEAVE is grouped under Controls in windows-sys; only this message is
// needed, not the common-controls API feature.
const WM_MOUSELEAVE: u32 = 0x02A3;
static EVENT_OWNER: AtomicUsize = AtomicUsize::new(0);
static EVENT_QUEUED: AtomicBool = AtomicBool::new(false);
static POPUP_CARD: AtomicUsize = AtomicUsize::new(0);
static POPUP_RAIL: AtomicUsize = AtomicUsize::new(0);
static EVENT_HOST: AtomicUsize = AtomicUsize::new(0);
static EVENT_START: AtomicUsize = AtomicUsize::new(0);

pub fn rail_class(root: &Path) -> String {
    format!("{}.Rail", windows::window_class(root))
}
pub fn card_class(root: &Path) -> String {
    format!("{}.Card", windows::window_class(root))
}
fn error() -> ProbeError {
    ProbeError::new(
        ErrorKind::Io,
        "QuotaRail widget could not be created; the tray remains available.",
    )
}
fn geometry(rect: RECT) -> Rect {
    Rect {
        left: rect.left,
        top: rect.top,
        right: rect.right,
        bottom: rect.bottom,
    }
}
fn window_rect(hwnd: HWND) -> Rect {
    let mut r = unsafe { zeroed() };
    unsafe {
        GetWindowRect(hwnd, &mut r);
    }
    geometry(r)
}
fn dpi(hwnd: HWND) -> u32 {
    unsafe { GetDpiForWindow(hwnd) }.max(96)
}
fn move_window(hwnd: HWND, r: Rect) {
    if window_rect(hwnd) == r {
        return;
    }
    unsafe {
        SetWindowPos(
            hwnd,
            null_mut(),
            r.left,
            r.top,
            r.width(),
            r.height(),
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

#[derive(Clone)]
struct Monitor {
    handle: HMONITOR,
    name: String,
    work: Rect,
    screen: Rect,
    dpi: u32,
}
fn monitor(handle: HMONITOR) -> Option<Monitor> {
    let mut info: MONITORINFOEXW = unsafe { zeroed() };
    info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
    if unsafe { GetMonitorInfoW(handle, (&mut info as *mut MONITORINFOEXW).cast()) } == 0 {
        return None;
    }
    let end = info
        .szDevice
        .iter()
        .position(|n| *n == 0)
        .unwrap_or(info.szDevice.len());
    let (mut x, mut y) = (96, 96);
    unsafe {
        GetDpiForMonitor(handle, MDT_EFFECTIVE_DPI, &mut x, &mut y);
    }
    Some(Monitor {
        handle,
        name: String::from_utf16_lossy(&info.szDevice[..end]),
        work: geometry(info.monitorInfo.rcWork),
        screen: geometry(info.monitorInfo.rcMonitor),
        dpi: x.max(96),
    })
}
unsafe extern "system" fn collect_monitor(h: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> i32 {
    if let Some(m) = monitor(h) {
        unsafe {
            (&mut *(data as *mut Vec<Monitor>)).push(m);
        }
    }
    1
}
fn monitors() -> Vec<Monitor> {
    let mut result = Vec::new();
    unsafe {
        EnumDisplayMonitors(
            null_mut(),
            null(),
            Some(collect_monitor),
            (&mut result as *mut Vec<Monitor>) as LPARAM,
        );
    }
    result
}
fn current_monitor(hwnd: HWND) -> Option<Monitor> {
    monitor(unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) })
}

#[derive(Clone, Copy)]
struct Drag {
    start: POINT,
    origin: Rect,
    moved: bool,
}
struct Context {
    owner: HWND,
    rail: Cell<HWND>,
    card: Cell<HWND>,
    shared: SharedState,
    root: PathBuf,
    config: RefCell<WidgetConfig>,
    warning: Cell<bool>,
    palette: RefCell<Palette>,
    dark: Cell<bool>,
    theme_initialized: Cell<bool>,
    drag: RefCell<Option<Drag>>,
    last_text: RefCell<String>,
    suppressed: Cell<bool>,
    click_hook: Cell<HHOOK>,
    taskbar: Option<u32>,
    host: Cell<Option<Host>>,
    host_error: Cell<Option<&'static str>>,
}
struct WindowData {
    context: Rc<Context>,
    card: bool,
    canvas: RefCell<Option<Canvas>>,
    focus: Cell<usize>,
    alive: Cell<bool>,
    hovered: Cell<bool>,
}
struct OwnedWindow {
    hwnd: HWND,
    _data: Box<WindowData>,
}
impl Drop for OwnedWindow {
    fn drop(&mut self) {
        unsafe {
            if self._data.alive.get() {
                DestroyWindow(self.hwnd);
            }
        }
    }
}
struct Com(bool);
impl Drop for Com {
    fn drop(&mut self) {
        if self.0 {
            unsafe {
                CoUninitialize();
            }
        }
    }
}
struct Hooks(Vec<HWINEVENTHOOK>);
impl Drop for Hooks {
    fn drop(&mut self) {
        EVENT_OWNER.store(0, Ordering::Relaxed);
        for h in &self.0 {
            unsafe {
                UnhookWinEvent(*h);
            }
        }
    }
}
pub struct Widget {
    _hooks: Hooks,
    windows: RefCell<Vec<OwnedWindow>>,
    context: Rc<Context>,
    _com: Com,
}

impl Widget {
    pub fn new(
        owner: HWND,
        shared: SharedState,
        root: &Path,
        taskbar: Option<u32>,
    ) -> Result<Self> {
        let com = Com(unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok());
        let loaded = WidgetConfig::load(root);
        let context = Rc::new(Context {
            owner,
            rail: Cell::new(null_mut()),
            card: Cell::new(null_mut()),
            shared,
            root: root.into(),
            warning: Cell::new(loaded.is_err()),
            config: RefCell::new(loaded.unwrap_or_default()),
            palette: RefCell::new(Palette::standard(false)),
            dark: Cell::new(false),
            theme_initialized: Cell::new(false),
            drag: RefCell::new(None),
            last_text: RefCell::new(String::new()),
            suppressed: Cell::new(false),
            click_hook: Cell::new(null_mut()),
            taskbar,
            host: Cell::new(None),
            host_error: Cell::new(None),
        });
        let rail = create_window(&rail_class(root), context.clone(), false)?;
        context.rail.set(rail.hwnd);
        let card = create_window(&card_class(root), context.clone(), true)?;
        context.card.set(card.hwnd);
        // Initialize the small renderer before claiming success. Card resources are lazy.
        *rail._data.canvas.borrow_mut() = Some(Canvas::new(rail.hwnd, true).map_err(|_| error())?);
        if taskbar.is_none() {
            unsafe {
                SetWindowPos(
                    rail.hwnd,
                    HWND_TOPMOST,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                );
            }
        }
        EVENT_OWNER.store(owner as usize, Ordering::Relaxed);
        let mut hooks = Vec::new();
        for event in [EVENT_SYSTEM_FOREGROUND, EVENT_OBJECT_LOCATIONCHANGE] {
            let h = unsafe {
                SetWinEventHook(
                    event,
                    event,
                    null_mut(),
                    Some(environment_event),
                    0,
                    0,
                    WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
                )
            };
            if !h.is_null() {
                hooks.push(h);
            }
        }
        let widget = Self {
            _hooks: Hooks(hooks),
            windows: RefCell::new(vec![card, rail]),
            context,
            _com: com,
        };
        widget.environment();
        widget.update();
        Ok(widget)
    }
    pub fn config(&self) -> WidgetConfig {
        self.context.config.borrow().clone()
    }
    pub fn apply_config(&self, config: WidgetConfig) -> std::io::Result<()> {
        config.save(&self.context.root)?;
        *self.context.config.borrow_mut() = config;
        self.context.warning.set(false);
        self.environment();
        Ok(())
    }
    pub fn taskbar_status(&self) -> Option<&'static str> {
        self.context
            .taskbar
            .map(|_| self.context.host_error.get().unwrap_or("已嵌入任务栏"))
    }
    pub fn update(&self) {
        self.context.update();
    }
    pub fn show_details(&self) {
        self.context.open_card(false);
    }
    pub fn dialog_owner(&self) -> HWND {
        self.context.hide_card();
        if self.context.taskbar.is_none()
            && unsafe { IsWindowVisible(self.context.rail.get()) } != 0
        {
            self.context.rail.get()
        } else {
            self.context.owner
        }
    }
    pub fn restore_after_dialog(&self) {
        self.context.raise_rail();
    }
    pub fn environment(&self) {
        EVENT_QUEUED.store(false, Ordering::Relaxed);
        if self.context.rail.get().is_null() {
            self.context.hide_card();
            match create_window(&rail_class(&self.context.root), self.context.clone(), false) {
                Ok(rail) => {
                    self.context.rail.set(rail.hwnd);
                    let mut windows = self.windows.borrow_mut();
                    windows.retain(|w| w._data.card);
                    windows.push(rail);
                    self.context.last_text.borrow_mut().clear();
                    self.context.theme_initialized.set(false);
                }
                Err(_) => {
                    self.context
                        .host_error
                        .set(Some("用量窗口重建失败；仅托盘"));
                    return;
                }
            }
        }
        self.context.theme();
        if self.context.drag.borrow().is_none() {
            self.context.restore();
        }
        self.context.visibility();
        self.context.update();
    }
    pub fn command(&self, action: u32) -> bool {
        match action {
            CMD_REBIND if self.context.taskbar.is_some() => {
                self.context.hide_card();
                let rail = self.context.rail.get();
                if !rail.is_null() {
                    unsafe {
                        DestroyWindow(rail);
                    }
                }
                self.environment();
                return true;
            }
            CMD_VISIBLE => {
                let mut c = self.context.config.borrow_mut();
                c.visible = !c.visible;
            }
            CMD_LOCK => {
                let mut c = self.context.config.borrow_mut();
                c.locked = !c.locked;
            }
            CMD_RESET => {
                let mut c = self.context.config.borrow_mut();
                if self.context.taskbar.is_some() {
                    c.taskbar_offset_dip = 0.0;
                } else {
                    c.placement = None;
                }
                c.visible = true;
            }
            CMD_THEME_SYSTEM | CMD_THEME_LIGHT | CMD_THEME_DARK => {
                self.context.config.borrow_mut().theme = match action {
                    CMD_THEME_LIGHT => Theme::Light,
                    CMD_THEME_DARK => Theme::Dark,
                    _ => Theme::System,
                }
            }
            _ => return false,
        }
        self.context.save();
        self.environment();
        true
    }
}

impl Context {
    fn raise_rail(&self) {
        if unsafe { IsWindowVisible(self.rail.get()) } == 0 {
            return;
        }
        let after = if self.taskbar.is_some() {
            HWND_TOP
        } else {
            HWND_TOPMOST
        };
        unsafe {
            SetWindowPos(
                self.rail.get(),
                after,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
    }
    fn move_rail(&self, r: Rect) {
        if let Some(host) = self.host.get() {
            let left = r.left.clamp(host.slot.left, host.slot.right - r.width());
            let mut target = host.position(0.0);
            target.left = left;
            target.right = left + r.width();
            host.place(self.rail.get(), target);
        } else if self.taskbar.is_none() {
            move_window(self.rail.get(), r);
        }
    }
    fn save(&self) {
        self.warning
            .set(self.config.borrow().save(&self.root).is_err());
    }
    fn restore(&self) {
        if let Some(left_space) = self.taskbar {
            let result = Host::discover(left_space).and_then(|host| {
                EVENT_HOST.store(host.hwnd as usize, Ordering::Relaxed);
                EVENT_START.store(host.start as usize, Ordering::Relaxed);
                if unsafe { GetParent(self.rail.get()) } != host.hwnd {
                    host.attach(self.rail.get(), self.owner)?;
                }
                host.place(
                    self.rail.get(),
                    host.position(self.config.borrow().taskbar_offset_dip),
                );
                self.host.set(Some(host));
                Ok(())
            });
            self.host_error.set(result.err());
            if self.host_error.get().is_some() {
                self.host.set(None);
                taskbar::detach(self.rail.get(), self.owner);
                self.hide_card();
            }
            self.position_card();
            return;
        }
        let items = monitors();
        let selected = self
            .config
            .borrow()
            .placement
            .as_ref()
            .and_then(|p| items.iter().find(|m| m.name == p.monitor))
            .cloned()
            .or_else(|| {
                monitor(unsafe { MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY) })
            });
        let Some(m) = selected else {
            return;
        };
        let placement = self.config.borrow().placement.clone();
        // A missing monitor loses its old anchor/offset and returns to a visible default.
        let r = placement
            .filter(|p| p.monitor == m.name)
            .map(|p| p.rect(m.screen, m.dpi))
            .unwrap_or_else(|| default_rect(m.work, m.dpi));
        if window_rect(self.rail.get()) != r {
            move_window(self.rail.get(), r);
        }
        self.position_card();
    }
    fn position_card(&self) {
        if let Some(m) = current_monitor(self.rail.get()) {
            move_window(
                self.card.get(),
                card_rect(window_rect(self.rail.get()), m.work, dpi(self.rail.get())),
            );
        }
    }
    fn save_position(&self) {
        if let Some(host) = self.host.get() {
            self.config.borrow_mut().taskbar_offset_dip =
                f64::from((host.slot.right - window_rect(self.rail.get()).right).max(0)) * 96.0
                    / f64::from(host.dpi);
            self.save();
            self.position_card();
            return;
        }
        let Some(m) = current_monitor(self.rail.get()) else {
            return;
        };
        let r = snap_over_taskbar(
            window_rect(self.rail.get()),
            m.screen,
            m.work,
            dpi(self.rail.get()),
        );
        move_window(self.rail.get(), r);
        self.config.borrow_mut().placement = Some(Placement::capture(
            m.name,
            r,
            m.screen,
            dpi(self.rail.get()),
        ));
        self.save();
        self.position_card();
        self.raise_rail();
    }
    fn hide_card(&self) {
        let was_visible = unsafe { IsWindowVisible(self.card.get()) } != 0;
        POPUP_CARD.store(0, Ordering::Relaxed);
        POPUP_RAIL.store(0, Ordering::Relaxed);
        let hook = self.click_hook.replace(null_mut());
        if !hook.is_null() {
            unsafe {
                UnhookWindowsHookEx(hook);
            }
        }
        unsafe {
            KillTimer(self.card.get(), CARD_TIMER);
            ShowWindow(self.card.get(), SW_HIDE);
        }
        if was_visible && unsafe { IsWindowVisible(self.rail.get()) } != 0 {
            // Closing an activated popup may bring Explorer above our non-activating
            // rail. Restore hit testing once for this user action, without polling.
            self.raise_rail();
        }
    }
    fn open_card(&self, toggle: bool) {
        let card = self.card.get();
        if toggle && unsafe { IsWindowVisible(card) } != 0 {
            self.hide_card();
            return;
        }
        self.position_card();
        self.update();
        unsafe {
            ShowWindow(card, SW_SHOWNORMAL);
            if self.taskbar.is_some() {
                SetWindowPos(
                    card,
                    HWND_TOPMOST,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                );
            }
            SetForegroundWindow(card);
            SetFocus(card);
            SetTimer(card, CARD_TIMER, 1000, None);
            InvalidateRect(card, null(), 0);
            // Keep the non-activating rail hittable after this explicit foreground
            // transition; do not repeat the raise from environment notifications.
        }
        self.raise_rail();
        if self.click_hook.get().is_null() && unsafe { IsWindowVisible(card) } != 0 {
            POPUP_CARD.store(card as usize, Ordering::Relaxed);
            POPUP_RAIL.store(self.rail.get() as usize, Ordering::Relaxed);
            self.click_hook.set(unsafe {
                SetWindowsHookExW(
                    WH_MOUSE_LL,
                    Some(outside_click),
                    GetModuleHandleW(null()),
                    0,
                )
            });
        }
    }
    fn update(&self) {
        let state = self
            .shared
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let now = unix_now();
        let text = super::view::details(&state, now, windows::local_time);
        if *self.last_text.borrow() != text {
            *self.last_text.borrow_mut() = text.clone();
            let title = wide(&super::view::tooltip(&state, now));
            let details = wide(&format!("QuotaRail · 额度详情\n{text}"));
            unsafe {
                SetWindowTextW(self.rail.get(), title.as_ptr());
                SetWindowTextW(self.card.get(), details.as_ptr());
                InvalidateRect(self.rail.get(), null(), 0);
                InvalidateRect(self.card.get(), null(), 0);
            }
        }
    }
    fn visibility(&self) {
        if self.taskbar.is_some() {
            // Follow the parent taskbar's visibility/fullscreen policy. A child
            // does not need a competing global TOPMOST/fullscreen heuristic.
            let visible = self.config.borrow().visible
                && self
                    .host
                    .get()
                    .is_some_and(|h| h.valid() && unsafe { IsWindowVisible(h.hwnd) } != 0);
            if (unsafe { IsWindowVisible(self.rail.get()) } != 0) != visible {
                unsafe {
                    ShowWindow(
                        self.rail.get(),
                        if visible { SW_SHOWNOACTIVATE } else { SW_HIDE },
                    );
                    if visible {
                        // A recreated layered child is visible before its first
                        // queued WM_PAINT. Present its alpha hit surface now.
                        InvalidateRect(self.rail.get(), null(), 0);
                        UpdateWindow(self.rail.get());
                    }
                }
            }
            if !visible {
                self.hide_card();
            }
            return;
        }
        let mut fullscreen = false;
        let foreground = unsafe { GetForegroundWindow() };
        let mut pid = 0;
        unsafe {
            GetWindowThreadProcessId(foreground, &mut pid);
        }
        if !foreground.is_null()
            && pid != std::process::id()
            && foreground != unsafe { GetShellWindow() }
            && foreground != unsafe { GetDesktopWindow() }
            && let Some(m) = current_monitor(self.rail.get())
            && unsafe { MonitorFromWindow(foreground, MONITOR_DEFAULTTONEAREST) } == m.handle
        {
            let r = window_rect(foreground);
            fullscreen = r.left <= m.screen.left
                && r.top <= m.screen.top
                && r.right >= m.screen.right
                && r.bottom >= m.screen.bottom;
        }
        self.suppressed.set(fullscreen);
        let visible = self.config.borrow().visible && !fullscreen;
        if (unsafe { IsWindowVisible(self.rail.get()) } != 0) != visible {
            unsafe {
                ShowWindow(
                    self.rail.get(),
                    if visible { SW_SHOWNOACTIVATE } else { SW_HIDE },
                );
            }
        }
        if !visible {
            self.hide_card();
        }
    }
    fn theme(&self) {
        let mode = self.config.borrow().theme;
        let mut light = 1u32;
        let mut size = 4u32;
        unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                wide("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize").as_ptr(),
                wide("AppsUseLightTheme").as_ptr(),
                RRF_RT_REG_DWORD,
                null_mut(),
                (&mut light as *mut u32).cast(),
                &mut size,
            );
        }
        let dark = match mode {
            Theme::System => light == 0,
            Theme::Light => false,
            Theme::Dark => true,
        };
        let mut p = Palette::standard(dark);
        let mut contrast: HIGHCONTRASTW = unsafe { zeroed() };
        contrast.cbSize = size_of::<HIGHCONTRASTW>() as u32;
        if unsafe {
            SystemParametersInfoW(
                SPI_GETHIGHCONTRAST,
                contrast.cbSize,
                (&mut contrast as *mut HIGHCONTRASTW).cast(),
                0,
            )
        } != 0
            && contrast.dwFlags & HCF_HIGHCONTRASTON != 0
        {
            let rgb = |index| {
                let c = unsafe { GetSysColor(index) };
                ((c & 255) << 16) | (c & 0xFF00) | ((c >> 16) & 255)
            };
            p.background = rgb(COLOR_WINDOW);
            p.text = rgb(COLOR_WINDOWTEXT);
            p.muted = p.text;
            p.track = rgb(COLOR_BTNFACE);
            p.blue = rgb(COLOR_HIGHLIGHT);
            p.green = p.blue;
            p.warning = p.text;
        }
        if self.theme_initialized.replace(true) && *self.palette.borrow() == p {
            return;
        }
        *self.palette.borrow_mut() = p;
        self.dark.set(dark);
        for hwnd in [self.rail.get(), self.card.get()] {
            let corner = if hwnd == self.rail.get() {
                DWMWCP_DONOTROUND
            } else {
                DWMWCP_ROUND
            };
            let dark_value = i32::from(dark);
            unsafe {
                DwmSetWindowAttribute(
                    hwnd,
                    DWMWA_WINDOW_CORNER_PREFERENCE as u32,
                    (&corner as *const DWM_WINDOW_CORNER_PREFERENCE).cast(),
                    size_of::<DWM_WINDOW_CORNER_PREFERENCE>() as u32,
                );
                DwmSetWindowAttribute(
                    hwnd,
                    DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
                    (&dark_value as *const i32).cast(),
                    4,
                );
                InvalidateRect(hwnd, null(), 0);
            }
        }
    }
}

impl Drop for Context {
    fn drop(&mut self) {
        let hook = self.click_hook.get();
        if !hook.is_null() {
            unsafe {
                UnhookWindowsHookEx(hook);
            }
        }
        POPUP_CARD.store(0, Ordering::Relaxed);
        POPUP_RAIL.store(0, Ordering::Relaxed);
        EVENT_HOST.store(0, Ordering::Relaxed);
        EVENT_START.store(0, Ordering::Relaxed);
    }
}

// Active only while the popup is visible. Observe down events, never consume input,
// retain coordinates or inspect another app's data; the original click still reaches it.
unsafe extern "system" fn outside_click(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code >= 0
        && matches!(
            wp as u32,
            WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN
        )
    {
        let card = POPUP_CARD.load(Ordering::Relaxed) as HWND;
        if !card.is_null() {
            let point = unsafe { (*(lp as *const MSLLHOOKSTRUCT)).pt };
            let inside = |hwnd: HWND| {
                if hwnd.is_null() || unsafe { IsWindowVisible(hwnd) } == 0 {
                    return false;
                }
                let r = window_rect(hwnd);
                point.x >= r.left && point.x < r.right && point.y >= r.top && point.y < r.bottom
            };
            if !inside(card) && !inside(POPUP_RAIL.load(Ordering::Relaxed) as HWND) {
                unsafe {
                    PostMessageW(card, WM_CLOSE, 0, 0);
                }
            }
        }
    }
    unsafe { CallNextHookEx(null_mut(), code, wp, lp) }
}

unsafe extern "system" fn environment_event(
    _: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    object: i32,
    _: i32,
    _: u32,
    _: u32,
) {
    if event == EVENT_OBJECT_LOCATIONCHANGE
        && (object != 0
            || (hwnd != unsafe { GetForegroundWindow() }
                && hwnd as usize != EVENT_HOST.load(Ordering::Relaxed)
                && hwnd as usize != EVENT_START.load(Ordering::Relaxed)))
    {
        return;
    }
    let owner = EVENT_OWNER.load(Ordering::Relaxed) as HWND;
    if !owner.is_null() && !EVENT_QUEUED.swap(true, Ordering::Relaxed) {
        unsafe {
            PostMessageW(owner, WM_ENVIRONMENT, 0, 0);
        }
    }
}

fn create_window(class: &str, context: Rc<Context>, card: bool) -> Result<OwnedWindow> {
    let name = wide(class);
    let instance = unsafe { GetModuleHandleW(null()) };
    let wc = WNDCLASSW {
        lpfnWndProc: Some(window_proc),
        hInstance: instance,
        hCursor: unsafe { LoadCursorW(null_mut(), IDC_ARROW) },
        ..unsafe { zeroed() }
    };
    let wc = WNDCLASSW {
        lpszClassName: name.as_ptr(),
        ..wc
    };
    if unsafe { RegisterClassW(&wc) } == 0
        && unsafe { GetLastError() } != ERROR_CLASS_ALREADY_EXISTS
    {
        return Err(error());
    }
    let mut data = Box::new(WindowData {
        context: context.clone(),
        card,
        canvas: RefCell::new(None),
        focus: Cell::new(0),
        alive: Cell::new(true),
        hovered: Cell::new(false),
    });
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_TOOLWINDOW
                | if card {
                    0
                } else {
                    WS_EX_NOACTIVATE | WS_EX_LAYERED
                },
            name.as_ptr(),
            wide("QuotaRail").as_ptr(),
            WS_POPUP | if card { WS_THICKFRAME } else { 0 },
            0,
            0,
            1,
            1,
            if card && context.taskbar.is_none() {
                context.rail.get()
            } else {
                context.owner
            },
            null_mut(),
            instance,
            (&mut *data as *mut WindowData).cast(),
        )
    };
    if hwnd.is_null() {
        return Err(error());
    }
    Ok(OwnedWindow { hwnd, _data: data })
}

// The Box is owned by OwnedWindow until after DestroyWindow returns. Shared references
// use Cell/RefCell with short borrows; no mutable Rust reference crosses re-entrant Win32 calls.
unsafe extern "system" fn window_proc(hwnd: HWND, message: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if message == WM_NCCREATE {
        let create = unsafe { &*(lp as *const CREATESTRUCTW) };
        unsafe {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
        }
    }
    let pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const WindowData;
    if pointer.is_null() {
        return unsafe { DefWindowProcW(hwnd, message, wp, lp) };
    }
    let data = unsafe { &*pointer };
    let c = &data.context;
    match message {
        WM_NCCALCSIZE if wp != 0 => 0,
        WM_NCHITTEST => HTCLIENT as LRESULT,
        WM_ERASEBKGND => 1,
        WM_NCDESTROY => unsafe {
            data.alive.set(false);
            if !data.card && c.rail.get() == hwnd {
                c.rail.set(null_mut());
                c.host.set(None);
                PostMessageW(c.owner, WM_ENVIRONMENT, 0, 0);
            }
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            DefWindowProcW(hwnd, message, wp, lp)
        },
        WM_MOUSEACTIVATE if !data.card => MA_NOACTIVATE as LRESULT,
        WM_PAINT => {
            let mut paint: PAINTSTRUCT = unsafe { zeroed() };
            unsafe {
                BeginPaint(hwnd, &mut paint);
            }
            let mut rect: RECT = unsafe { zeroed() };
            unsafe {
                GetClientRect(hwnd, &mut rect);
            }
            let state = c.shared.lock().unwrap_or_else(|e| e.into_inner()).clone();
            let rendered = if let Ok(mut renderer) = data.canvas.try_borrow_mut() {
                if renderer.is_none() {
                    *renderer = Canvas::new(hwnd, !data.card).ok();
                }
                let result = renderer.as_ref().is_some_and(|p| {
                    p.paint(
                        &state,
                        Frame {
                            card: data.card,
                            palette: &c.palette.borrow(),
                            focus: data.focus.get(),
                            now: unix_now(),
                            size: [rect.right.max(1) as u32, rect.bottom.max(1) as u32],
                            dpi: dpi(hwnd),
                            config_warning: c.warning.get(),
                            hovered: data.hovered.get(),
                        },
                    )
                    .is_ok()
                });
                if !result {
                    *renderer = None;
                }
                result
            } else {
                true
            };
            unsafe {
                EndPaint(hwnd, &paint);
            }
            if !rendered {
                unsafe {
                    ShowWindow(hwnd, SW_HIDE);
                }
            }
            0
        }
        WM_TIMER if data.card && wp == CARD_TIMER => {
            c.update();
            unsafe {
                InvalidateRect(hwnd, null(), 0);
            }
            0
        }
        WM_LBUTTONDOWN if !data.card => {
            let mut point = POINT { x: 0, y: 0 };
            unsafe {
                GetCursorPos(&mut point);
            }
            *c.drag.borrow_mut() = Some(Drag {
                start: point,
                origin: window_rect(hwnd),
                moved: false,
            });
            unsafe {
                SetCapture(hwnd);
            }
            0
        }
        WM_MOUSEMOVE if !data.card => {
            if !data.hovered.replace(true) {
                let mut tracking = TRACKMOUSEEVENT {
                    cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                unsafe {
                    if TrackMouseEvent(&mut tracking) == 0 {
                        data.hovered.set(false);
                    }
                    InvalidateRect(hwnd, null(), 0);
                }
            }
            let mut point = POINT { x: 0, y: 0 };
            unsafe {
                GetCursorPos(&mut point);
            }
            let locked = c.config.borrow().locked;
            let target = {
                let mut drag = c.drag.borrow_mut();
                drag.as_mut().and_then(|d| {
                    let dx = point.x - d.start.x;
                    let dy = point.y - d.start.y;
                    if !locked
                        && (d.moved
                            || dx.abs() >= px(4.0, dpi(hwnd))
                            || dy.abs() >= px(4.0, dpi(hwnd)))
                    {
                        d.moved = true;
                        Some(Rect {
                            left: d.origin.left + dx,
                            top: d.origin.top + dy,
                            right: d.origin.right + dx,
                            bottom: d.origin.bottom + dy,
                        })
                    } else {
                        None
                    }
                })
            };
            if let Some(r) = target {
                c.hide_card();
                c.move_rail(r);
            }
            0
        }
        WM_SHOWWINDOW if !data.card && wp == 0 => {
            data.hovered.set(false);
            unsafe { DefWindowProcW(hwnd, message, wp, lp) }
        }
        WM_MOUSELEAVE if !data.card => {
            if data.hovered.replace(false) {
                unsafe {
                    InvalidateRect(hwnd, null(), 0);
                }
            }
            0
        }
        WM_LBUTTONUP if !data.card => {
            let drag = c.drag.borrow_mut().take();
            unsafe {
                ReleaseCapture();
            }
            if let Some(d) = drag {
                if d.moved {
                    c.save_position();
                } else {
                    c.open_card(true);
                }
            }
            0
        }
        WM_CAPTURECHANGED if !data.card => {
            c.drag.borrow_mut().take();
            0
        }
        WM_LBUTTONUP if data.card => {
            let scale = 96.0 / f64::from(dpi(hwnd));
            let x = f64::from(lp as u16 as i16) * scale;
            let y = f64::from((lp >> 16) as u16 as i16) * scale;
            if (CARD_ACTION_TOP..CARD_ACTION_TOP + 30.0).contains(&y) {
                let action = if x < 184.0 {
                    windows::CMD_REFRESH
                } else {
                    windows::CMD_DETAILS
                };
                unsafe {
                    PostMessageW(c.owner, WM_COMMAND, action as usize, 1);
                }
            }
            0
        }
        WM_KEYDOWN if data.card => {
            match wp as u16 {
                VK_ESCAPE => c.hide_card(),
                VK_TAB => {
                    data.focus.set((data.focus.get() + 1) % 2);
                    unsafe {
                        InvalidateRect(hwnd, null(), 0);
                    }
                }
                VK_RETURN | VK_SPACE => {
                    let action = if data.focus.get() == 0 {
                        windows::CMD_REFRESH
                    } else {
                        windows::CMD_DETAILS
                    };
                    unsafe {
                        PostMessageW(c.owner, WM_COMMAND, action as usize, 1);
                    }
                }
                _ => {}
            }
            0
        }
        WM_ACTIVATE if data.card && wp & 0xFFFF == WA_INACTIVE as usize => {
            c.hide_card();
            0
        }
        WM_CLOSE => {
            if data.card {
                c.hide_card();
            } else {
                unsafe {
                    PostMessageW(c.owner, WM_COMMAND, CMD_VISIBLE as usize, 0);
                }
            }
            0
        }
        WM_CONTEXTMENU | WM_RBUTTONUP => {
            unsafe {
                PostMessageW(c.owner, windows::WM_MENU, 1, 0);
            }
            0
        }
        WM_DPICHANGED => {
            if !data.card && c.taskbar.is_some() {
                unsafe {
                    PostMessageW(c.owner, WM_ENVIRONMENT, 0, 0);
                }
                return 0;
            }
            let suggested = unsafe { *(lp as *const RECT) };
            let new_dpi = (wp & 0xFFFF) as u32;
            let (width, height) = if data.card {
                (CARD_WIDTH, CARD_HEIGHT)
            } else {
                (RAIL_WIDTH, RAIL_HEIGHT)
            };
            move_window(
                hwnd,
                Rect {
                    left: suggested.left,
                    top: suggested.top,
                    right: suggested.left + px(width, new_dpi),
                    bottom: suggested.top + px(height, new_dpi),
                },
            );
            unsafe {
                InvalidateRect(hwnd, null(), 0);
            }
            0
        }
        WM_SIZE => {
            unsafe {
                InvalidateRect(hwnd, null(), 0);
            }
            0
        }
        WM_DISPLAYCHANGE | WM_SETTINGCHANGE | WM_THEMECHANGED => {
            unsafe {
                PostMessageW(c.owner, WM_ENVIRONMENT, 0, 0);
            }
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wp, lp) },
    }
}
