//! Direct2D/DirectWrite renderer. No timers, input, configuration IO or Provider calls.
use super::{state::DesktopState, view, widget_surface::Surface, windows::local_time};
use windows::{
    Win32::Graphics::{
        Direct2D::{Common::*, *},
        DirectWrite::*,
    },
    core::w,
};

#[derive(PartialEq, Eq)]
pub struct Palette {
    pub background: u32,
    pub text: u32,
    pub muted: u32,
    pub track: u32,
    pub blue: u32,
    pub green: u32,
    pub warning: u32,
}

impl Palette {
    pub fn standard(dark: bool) -> Self {
        if dark {
            Self {
                background: 0x202329,
                text: 0xF2F4F8,
                muted: 0xA4ADBA,
                track: 0x3A414C,
                blue: 0x64A5FF,
                green: 0x42D9A2,
                warning: 0xFFBD66,
            }
        } else {
            Self {
                background: 0xFAFBFD,
                text: 0x1D2939,
                muted: 0x66758A,
                track: 0xE4E9F0,
                blue: 0x3487F8,
                green: 0x24C88D,
                warning: 0xB96A10,
            }
        }
    }
}
pub struct Canvas {
    target: ID2D1RenderTarget,
    surface: Surface,
    brush: ID2D1SolidColorBrush,
    small: IDWriteTextFormat,
    body: IDWriteTextFormat,
}
pub struct Frame<'a> {
    pub card: bool,
    pub palette: &'a Palette,
    pub focus: usize,
    pub now: u64,
    pub size: [u32; 2],
    pub dpi: u32,
    pub config_warning: bool,
    pub hovered: bool,
}
fn color(value: u32) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: ((value >> 16) & 255) as f32 / 255.0,
        g: ((value >> 8) & 255) as f32 / 255.0,
        b: (value & 255) as f32 / 255.0,
        a: 1.0,
    }
}
fn rect(x: f32, y: f32, width: f32, height: f32) -> D2D_RECT_F {
    D2D_RECT_F {
        left: x,
        top: y,
        right: x + width,
        bottom: y + height,
    }
}
impl Canvas {
    pub fn new(
        hwnd: windows_sys::Win32::Foundation::HWND,
        transparent: bool,
    ) -> windows::core::Result<Self> {
        unsafe {
            let factory: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let write: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let surface = Surface::new(&factory, hwnd, transparent)?;
            let target = surface.target.clone();
            let brush = target.CreateSolidColorBrush(&color(0), None)?;
            let format = |size, weight| -> windows::core::Result<IDWriteTextFormat> {
                let f = write.CreateTextFormat(
                    w!("Segoe UI Variable Text"),
                    None,
                    weight,
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    size,
                    w!("zh-CN"),
                )?;
                f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
                Ok(f)
            };
            Ok(Self {
                target,
                surface,
                brush,
                small: format(11.0, DWRITE_FONT_WEIGHT_NORMAL)?,
                body: format(12.0, DWRITE_FONT_WEIGHT_MEDIUM)?,
            })
        }
    }
    fn rounded(&self, x: f32, y: f32, width: f32, height: f32, radius: f32, value: u32) {
        if width <= 0.0 {
            return;
        }
        unsafe {
            self.brush.SetColor(&color(value));
            self.target.FillRoundedRectangle(
                &D2D1_ROUNDED_RECT {
                    rect: rect(x, y, width, height),
                    radiusX: radius,
                    radiusY: radius,
                },
                &self.brush,
            );
        }
    }
    fn text(&self, text: &str, bounds: D2D_RECT_F, value: u32, size: u8, right: bool) {
        let format = match size {
            0 => &self.small,
            _ => &self.body,
        };
        unsafe {
            self.brush.SetColor(&color(value));
            let _ = format.SetTextAlignment(if right {
                DWRITE_TEXT_ALIGNMENT_TRAILING
            } else {
                DWRITE_TEXT_ALIGNMENT_LEADING
            });
            self.target.DrawText(
                &text.encode_utf16().collect::<Vec<_>>(),
                format,
                &bounds,
                &self.brush,
                D2D1_DRAW_TEXT_OPTIONS_CLIP,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        }
    }
    pub fn paint(&self, state: &DesktopState, frame: Frame<'_>) -> windows::core::Result<()> {
        let Frame {
            card,
            palette,
            focus,
            now,
            size: [width, height],
            dpi,
            config_warning,
            hovered,
        } = frame;
        self.surface.prepare([width, height], dpi)?;
        unsafe {
            self.target.Clear(Some(&if card {
                color(palette.background)
            } else {
                D2D1_COLOR_F::default()
            }));
            if !card {
                // Alpha-zero layered pixels pass input through. Retain a 1/255
                // alpha interior hit area so gaps between text/bars remain draggable.
                // The outer 4 DIP are fully transparent; no visible plate or border.
                self.brush.SetColor(&D2D1_COLOR_F {
                    a: 1.0 / 255.0,
                    ..color(palette.background)
                });
                let scale = 96.0 / dpi as f32;
                self.target.FillRectangle(
                    &rect(
                        4.0,
                        4.0,
                        width as f32 * scale - 8.0,
                        height as f32 * scale - 8.0,
                    ),
                    &self.brush,
                );
            }
        }
        if card {
            self.card(state, palette, focus, now, config_warning);
        } else {
            self.rail(state, palette, now, hovered);
        }
        self.surface.present()
    }
    fn rail(&self, state: &DesktopState, p: &Palette, now: u64, hovered: bool) {
        let bucket = state.snapshot.as_ref().and_then(view::primary_bucket);
        for (i, minutes) in [300, 10080].iter().enumerate() {
            let y = 3.0 + i as f32 * 18.0;
            let window = bucket.and_then(|b| {
                b.windows
                    .iter()
                    .find(|w| w.window_duration_minutes == Some(*minutes))
            });
            self.text(
                if i == 0 { "5H" } else { "7D" },
                rect(8.0, y, 22.0, 17.0),
                p.muted,
                0,
                false,
            );
            let thickness = if hovered { 5.0 } else { 4.0 };
            let bar_y = y + 8.0 - thickness / 2.0;
            self.rounded(35.0, bar_y, 64.0, thickness, thickness / 2.0, p.track);
            if let Some(remaining) = window.and_then(view::remaining_percent) {
                self.rounded(
                    35.0,
                    bar_y,
                    64.0 * remaining as f32 / 100.0,
                    thickness,
                    thickness / 2.0,
                    if i == 0 { p.blue } else { p.green },
                );
            }
            let percent = window
                .map(|w| {
                    format!(
                        "{}{}",
                        view::remaining_percent(w)
                            .map(|p| format!("{p:.0}%"))
                            .unwrap_or_else(|| "—".into()),
                        if w.percent_out_of_range { "!" } else { "" }
                    )
                })
                .unwrap_or_else(|| "—".into());
            self.text(&percent, rect(107.0, y - 1.0, 39.0, 18.0), p.text, 1, true);
        }
        let status = view::status(state, now);
        if status != "正常" {
            self.rounded(
                149.0,
                4.0,
                4.0,
                4.0,
                2.0,
                if state.loading { p.blue } else { p.warning },
            );
        }
    }
    fn card(
        &self,
        state: &DesktopState,
        p: &Palette,
        focus: usize,
        now: u64,
        config_warning: bool,
    ) {
        let bucket = state.snapshot.as_ref().and_then(view::primary_bucket);
        let status = view::status(state, now);
        let short = if status == "正常" {
            "账户已核验"
        } else if state.error.is_some() && state.snapshot.is_some() {
            "刷新失败 · 保留上次数据"
        } else {
            status
        };
        let heading = match bucket {
            Some(b) if b.id != "codex" => format!("{short} · {}", b.id),
            _ => short.into(),
        };
        self.text(
            &heading,
            rect(20.0, 16.0, 328.0, 18.0),
            if status == "正常" {
                p.muted
            } else {
                p.warning
            },
            0,
            false,
        );
        for (i, minutes) in [300, 10080].iter().enumerate() {
            let y = 43.0 + i as f32 * 65.0;
            let window = bucket.and_then(|b| {
                b.windows
                    .iter()
                    .find(|w| w.window_duration_minutes == Some(*minutes))
            });
            self.text(
                if i == 0 { "5H" } else { "7D" },
                rect(20.0, y, 28.0, 20.0),
                if i == 0 { p.blue } else { p.green },
                1,
                false,
            );
            let value = window.map(view::window_values).unwrap_or_else(|| {
                if bucket.is_some_and(|b| {
                    b.windows
                        .iter()
                        .any(|w| w.window_duration_minutes.is_none())
                }) {
                    "窗口时长未知".into()
                } else if state.snapshot.is_some() {
                    "本次未提供".into()
                } else {
                    "尚无已核验数据".into()
                }
            });
            self.text(&value, rect(54.0, y, 294.0, 20.0), p.text, 1, true);
            let reset = window.and_then(|w| w.resets_at_unix_seconds);
            self.text("重置", rect(20.0, y + 22.0, 40.0, 17.0), p.muted, 0, false);
            self.text(
                &reset.map(local_time).unwrap_or_else(|| "未提供".into()),
                rect(64.0, y + 22.0, 284.0, 18.0),
                p.muted,
                0,
                true,
            );
            let countdown = reset
                .map(|t| {
                    let s = u64::try_from(t).unwrap_or_default().saturating_sub(now);
                    if s == 0 {
                        "已到时，等待服务端更新".into()
                    } else if s >= 86400 {
                        format!(
                            "{}天 {}小时 {:02}分后重置",
                            s / 86400,
                            s % 86400 / 3600,
                            s % 3600 / 60
                        )
                    } else {
                        format!(
                            "{}小时 {:02}分 {:02}秒后重置",
                            s / 3600,
                            s % 3600 / 60,
                            s % 60
                        )
                    }
                })
                .unwrap_or_else(|| "重置时间未知".into());
            self.text(
                &countdown,
                rect(64.0, y + 39.0, 284.0, 18.0),
                p.muted,
                0,
                true,
            );
        }
        self.rounded(20.0, 175.0, 328.0, 1.0, 0.0, p.track);
        self.text("上次成功", rect(20.0, 187.0, 70.0, 18.0), p.muted, 0, false);
        let fetched = state
            .snapshot
            .as_ref()
            .map(|s| local_time(s.fetched_at_unix_seconds as i64))
            .unwrap_or_else(|| "尚无成功读取".into());
        self.text(&fetched, rect(91.0, 186.0, 257.0, 20.0), p.text, 1, true);
        let source = if config_warning {
            "位置设置未能保存；可继续使用".into()
        } else {
            state
                .snapshot
                .as_ref()
                .map(|s| {
                    format!(
                        "CLI {} · 自动约{}分钟 · {}",
                        s.source.cli_version,
                        state.refresh_minutes.max(5),
                        super::windows::local_zone()
                    )
                })
                .unwrap_or_else(|| "Codex App Server · 可右键重新连接".into())
        };
        self.text(&source, rect(20.0, 210.0, 328.0, 18.0), p.muted, 0, false);
        let refresh = if state.loading {
            "刷新中…".into()
        } else if state.manual_available_at > now {
            format!("{}秒后可刷新", state.manual_available_at - now)
        } else {
            "刷新额度".into()
        };
        let can_refresh = super::state::manual_allowed(now, state);
        for (i, label) in [refresh.as_str(), "全部详情"].iter().enumerate() {
            let x = 20.0 + i as f32 * 168.0;
            let selected = focus == i && (i != 0 || can_refresh);
            self.rounded(
                x,
                super::widget_layout::CARD_ACTION_TOP as f32,
                160.0,
                26.0,
                5.0,
                if selected { p.blue } else { p.track },
            );
            self.text(
                label,
                rect(
                    x + 12.0,
                    super::widget_layout::CARD_ACTION_TOP as f32 + 4.0,
                    136.0,
                    19.0,
                ),
                if i == 0 && !can_refresh {
                    p.muted
                } else if selected {
                    p.background
                } else {
                    p.text
                },
                1,
                false,
            );
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::desktop::widget_layout::{RAIL_HEIGHT, RAIL_WIDTH};
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    #[test]
    fn rail_has_transparent_edges_legible_ink_and_a_nearly_invisible_drag_area() {
        unsafe {
            let (width, height) = (RAIL_WIDTH as usize, RAIL_HEIGHT as usize);
            let hwnd = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TOOLWINDOW,
                windows::core::w!("STATIC").as_ptr(),
                null(),
                WS_POPUP,
                0,
                0,
                width as i32,
                height as i32,
                null_mut(),
                null_mut(),
                null_mut(),
                null(),
            );
            assert!(!hwnd.is_null());
            struct Window(windows_sys::Win32::Foundation::HWND);
            impl Drop for Window {
                fn drop(&mut self) {
                    unsafe {
                        DestroyWindow(self.0);
                    }
                }
            }
            let _window = Window(hwnd);
            let canvas = Canvas::new(hwnd, true).unwrap();
            for (dark, hovered) in [(false, false), (false, true), (true, false), (true, true)] {
                let palette = Palette::standard(dark);
                canvas
                    .paint(
                        &DesktopState::default(),
                        Frame {
                            card: false,
                            palette: &palette,
                            focus: 0,
                            now: 0,
                            size: [width as u32, height as u32],
                            dpi: 96,
                            config_warning: false,
                            hovered,
                        },
                    )
                    .unwrap();
                let pixels = canvas.surface.pixels();
                let alpha = |x: usize, y: usize| pixels[(y * width + x) * 4 + 3];
                for x in 0..width {
                    assert_eq!(alpha(x, 0), 0);
                    assert_eq!(alpha(x, height - 1), 0);
                }
                for y in 0..height {
                    assert_eq!(alpha(0, y), 0);
                    assert_eq!(alpha(width - 1, y), 0);
                }
                assert_eq!(
                    alpha(100, 20),
                    1,
                    "gaps remain draggable without an opaque background"
                );
                assert_eq!(
                    alpha(60, 11),
                    255,
                    "the bar is not faded with the background"
                );
                assert_eq!(
                    alpha(60, 8) > 1,
                    hovered,
                    "hover expands the bar without adding a plate"
                );
                assert!(
                    pixels
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .any(|p| p[3] > 32 && p[3] < 255),
                    "antialiased text and edges retain per-pixel alpha"
                );
                assert!(
                    pixels
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .all(|p| p[..3].iter().all(|v| *v <= p[3])),
                    "BGRA must be premultiplied to avoid bright fringes"
                );
                // Optional local visual QA: export our renderer's surface only,
                // with synthetic quota. No screenshots or live account values.
                if let Some(directory) = std::env::var_os("QUOTARAIL_UI_PREVIEW_DIR") {
                    let response: crate::wire::RateLimitsResponse =
                        serde_json::from_value(serde_json::json!({
                            "rateLimits": {"primary": {"usedPercent":42,"windowDurationMins":300},
                                "secondary": {"usedPercent":68,"windowDurationMins":10080}}
                        }))
                        .unwrap();
                    let state = DesktopState {
                        snapshot: Some(
                            response
                                .normalize("synthetic-preview".into(), 1000)
                                .unwrap(),
                        ),
                        ..Default::default()
                    };
                    canvas
                        .paint(
                            &state,
                            Frame {
                                card: false,
                                palette: &palette,
                                focus: 0,
                                now: 1000,
                                size: [237, 60],
                                dpi: 144,
                                config_warning: false,
                                hovered,
                            },
                        )
                        .unwrap();
                    let path = std::path::Path::new(&directory);
                    std::fs::create_dir_all(path).unwrap();
                    std::fs::write(
                        path.join(format!(
                            "rail-{}-{}.bgra",
                            if dark { "dark" } else { "light" },
                            if hovered { "hover" } else { "idle" }
                        )),
                        canvas.surface.pixels(),
                    )
                    .unwrap();
                }
            }
        }
    }
}
