//! Win11 primary taskbar host. Only our HWND is changed; Explorer
//! controls, procedures and memory are never written. Internal class names are
//! an explicitly version-dependent discovery mechanism, not a Shell contract.
use super::{
    widget_layout::{RAIL_HEIGHT, RAIL_WIDTH, Rect, px},
    windows::wide,
};
use std::{
    mem::{size_of, zeroed},
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    UI::{HiDpi::*, WindowsAndMessaging::*},
};

#[derive(Clone, Copy, Debug)]
pub struct Host {
    pub hwnd: HWND,
    pub start: HWND,
    pub slot: Rect,
    pub dpi: u32,
    pid: u32,
}
fn bounds(hwnd: HWND) -> Option<Rect> {
    let mut r = unsafe { zeroed() };
    (unsafe { GetWindowRect(hwnd, &mut r) } != 0).then_some(Rect {
        left: r.left,
        top: r.top,
        right: r.right,
        bottom: r.bottom,
    })
}
fn pid(hwnd: HWND) -> u32 {
    let mut p = 0;
    unsafe {
        GetWindowThreadProcessId(hwnd, &mut p);
    }
    p
}
/// Reserve the left Widgets region conservatively. This is configurable space,
/// not a measurement of the Widgets button or automatic taskbar space allocation.
pub fn slot(taskbar: Rect, start: Rect, dpi: u32, left_space: u32) -> Option<Rect> {
    let r = Rect {
        left: taskbar.left + px(f64::from(left_space), dpi),
        top: taskbar.top,
        right: start.left - px(8.0, dpi),
        bottom: taskbar.bottom,
    };
    (start.left >= taskbar.left
        && start.right <= taskbar.right
        && start.top >= taskbar.top
        && start.bottom <= taskbar.bottom
        && r.width() >= px(RAIL_WIDTH, dpi)
        && r.height() >= px(RAIL_HEIGHT, dpi))
    .then_some(r)
}
impl Host {
    pub fn discover(left_space: u32) -> Result<Self, &'static str> {
        unsafe {
            let hwnd = FindWindowW(wide("Shell_TrayWnd").as_ptr(), null());
            let shell = GetShellWindow();
            if hwnd.is_null() || shell.is_null() || pid(hwnd) == 0 || pid(hwnd) != pid(shell) {
                return Err("未找到系统任务栏");
            }
            let start_window = FindWindowExW(hwnd, null_mut(), wide("Start").as_ptr(), null());
            let bar = bounds(hwnd).ok_or("无法读取任务栏位置")?;
            let start = bounds(start_window).ok_or("未识别开始按钮布局")?;
            let dpi = GetDpiForWindow(hwnd);
            let mut info: MONITORINFO = zeroed();
            info.cbSize = size_of::<MONITORINFO>() as u32;
            if dpi == 0
                || GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTOPRIMARY), &mut info)
                    == 0
                || bar.width() < bar.height()
                || bar.bottom != info.rcMonitor.bottom
            {
                return Err("当前仅支持主屏底部任务栏");
            }
            let slot = slot(bar, start, dpi, left_space).ok_or("天气与开始按钮之间空间不足")?;
            Ok(Self {
                hwnd,
                start: start_window,
                slot,
                dpi,
                pid: pid(hwnd),
            })
        }
    }
    pub fn valid(self) -> bool {
        unsafe { IsWindow(self.hwnd) != 0 && pid(self.hwnd) == self.pid }
    }
    pub fn position(self, offset_dip: f64) -> Rect {
        let width = px(RAIL_WIDTH, self.dpi);
        let height = px(RAIL_HEIGHT, self.dpi);
        let left = (self.slot.right - width - px(offset_dip, self.dpi))
            .clamp(self.slot.left, self.slot.right - width);
        let top = self.slot.top + (self.slot.height() - height) / 2;
        Rect {
            left,
            top,
            right: left + width,
            bottom: top + height,
        }
    }
    /// Preflight DPI before reparenting: cross-process mismatch can reset our
    /// process DPI mode. A failure leaves our rail hidden and detached.
    pub(crate) fn attach(self, rail: HWND, owner: HWND) -> Result<(), &'static str> {
        unsafe {
            if !self.valid() || pid(rail) != std::process::id() {
                return Err("宿主窗口失效");
            }
            let before = GetWindowDpiAwarenessContext(rail);
            if AreDpiAwarenessContextsEqual(before, GetWindowDpiAwarenessContext(self.hwnd)) == 0 {
                return Err("任务栏DPI模式不兼容");
            }
            ShowWindow(rail, SW_HIDE);
            // This foreign parent must not receive our child creation/mouse
            // WM_PARENTNOTIFY messages as if they were Shell controls.
            let ex = GetWindowLongPtrW(rail, GWL_EXSTYLE) as u32;
            SetWindowLongPtrW(rail, GWL_EXSTYLE, (ex | WS_EX_NOPARENTNOTIFY) as isize);
            SetWindowLongPtrW(rail, GWLP_HWNDPARENT, 0);
            let style = GetWindowLongPtrW(rail, GWL_STYLE) as u32;
            SetWindowLongPtrW(
                rail,
                GWL_STYLE,
                ((style & !WS_POPUP) | WS_CHILD | WS_CLIPSIBLINGS) as isize,
            );
            SetWindowPos(
                rail,
                HWND_NOTOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
            SetLastError(0);
            SetParent(rail, self.hwnd);
            if GetParent(rail) != self.hwnd
                || GetWindowLongPtrW(rail, GWL_STYLE) as u32 & WS_CHILD == 0
                || AreDpiAwarenessContextsEqual(before, GetWindowDpiAwarenessContext(rail)) == 0
            {
                detach(rail, owner);
                return Err("子窗口绑定失败");
            }
            self.place(rail, self.position(0.0));
            Ok(())
        }
    }
    pub(crate) fn place(self, rail: HWND, r: Rect) {
        if !self.valid()
            || pid(rail) != std::process::id()
            || unsafe { GetParent(rail) } != self.hwnd
            || bounds(rail) == Some(r)
        {
            return;
        }
        let mut point = POINT {
            x: r.left,
            y: r.top,
        };
        unsafe {
            ScreenToClient(self.hwnd, &mut point);
            SetWindowPos(
                rail,
                HWND_TOP,
                point.x,
                point.y,
                r.width(),
                r.height(),
                SWP_NOACTIVATE,
            );
        }
    }
}
/// Detach only a window owned by this process. Never destroy an external parent.
pub(crate) fn detach(rail: HWND, owner: HWND) {
    if rail.is_null() || pid(rail) != std::process::id() {
        return;
    }
    unsafe {
        ShowWindow(rail, SW_HIDE);
        SetParent(rail, null_mut());
        let style = GetWindowLongPtrW(rail, GWL_STYLE) as u32;
        SetWindowLongPtrW(rail, GWL_STYLE, ((style & !WS_CHILD) | WS_POPUP) as isize);
        let ex = GetWindowLongPtrW(rail, GWL_EXSTYLE) as u32;
        SetWindowLongPtrW(rail, GWL_EXSTYLE, (ex & !WS_EX_NOPARENTNOTIFY) as isize);
        SetWindowLongPtrW(rail, GWLP_HWNDPARENT, owner as isize);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn gap_layout_rejects_crowded_and_left_aligned_bars_and_clamps_dragging() {
        let bar = Rect {
            left: 0,
            top: 1528,
            right: 2560,
            bottom: 1600,
        };
        let start = Rect {
            left: 782,
            top: 1528,
            right: 849,
            bottom: 1600,
        };
        let s = slot(bar, start, 144, 240).unwrap();
        assert_eq!(s.left, 360);
        assert_eq!(s.right, 770);
        let h = Host {
            hwnd: null_mut(),
            start: null_mut(),
            slot: s,
            dpi: 144,
            pid: 0,
        };
        assert_eq!(h.position(0.0).left, 533);
        assert_eq!(h.position(9999.0).left, s.left);
        assert!(
            slot(
                bar,
                Rect {
                    left: 0,
                    right: 67,
                    ..start
                },
                144,
                240
            )
            .is_none()
        );
        assert!(
            slot(
                bar,
                Rect {
                    left: 600,
                    right: 667,
                    ..start
                },
                144,
                240
            )
            .is_none()
        );
    }
}
