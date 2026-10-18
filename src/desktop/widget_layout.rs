//! Pixel geometry and versioned DIP placement, independent of HWND and rendering.
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::Path,
};

pub const RAIL_WIDTH: f64 = 158.0;
pub const RAIL_HEIGHT: f64 = 40.0;
pub const CARD_WIDTH: f64 = 368.0;
pub const CARD_HEIGHT: f64 = 274.0;
pub const CARD_ACTION_TOP: f64 = 236.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}
impl Rect {
    pub fn width(self) -> i32 {
        self.right - self.left
    }
    pub fn height(self) -> i32 {
        self.bottom - self.top
    }
    pub fn clamp(self, work: Rect) -> Self {
        let width = self.width().min(work.width()).max(1);
        let height = self.height().min(work.height()).max(1);
        let left = self.left.clamp(work.left, work.right - width);
        let top = self.top.clamp(work.top, work.bottom - height);
        Self {
            left,
            top,
            right: left + width,
            bottom: top + height,
        }
    }
}
pub fn px(dip: f64, dpi: u32) -> i32 {
    (dip * f64::from(dpi) / 96.0).round() as i32
}

#[derive(Clone, Copy, Default, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Anchor {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Placement {
    pub monitor: String,
    pub anchor: Anchor,
    pub x_dip: f64,
    pub y_dip: f64,
}
impl Placement {
    pub fn rect(&self, work: Rect, dpi: u32) -> Rect {
        let width = px(RAIL_WIDTH, dpi);
        let height = px(RAIL_HEIGHT, dpi);
        let left = match self.anchor {
            Anchor::TopLeft | Anchor::BottomLeft => work.left + px(self.x_dip, dpi),
            _ => work.right - width - px(self.x_dip, dpi),
        };
        let top = match self.anchor {
            Anchor::TopLeft | Anchor::TopRight => work.top + px(self.y_dip, dpi),
            _ => work.bottom - height - px(self.y_dip, dpi),
        };
        Rect {
            left,
            top,
            right: left + width,
            bottom: top + height,
        }
        .clamp(work)
    }
    pub fn capture(monitor: String, rail: Rect, work: Rect, dpi: u32) -> Self {
        let left = i64::from(rail.left) + i64::from(rail.right)
            < i64::from(work.left) + i64::from(work.right);
        let top = i64::from(rail.top) + i64::from(rail.bottom)
            < i64::from(work.top) + i64::from(work.bottom);
        let anchor = match (left, top) {
            (true, true) => Anchor::TopLeft,
            (false, true) => Anchor::TopRight,
            (true, false) => Anchor::BottomLeft,
            _ => Anchor::BottomRight,
        };
        let scale = 96.0 / f64::from(dpi.max(1));
        Self {
            monitor,
            anchor,
            x_dip: f64::from(if left {
                rail.left - work.left
            } else {
                work.right - rail.right
            })
            .max(0.0)
                * scale,
            y_dip: f64::from(if top {
                rail.top - work.top
            } else {
                work.bottom - rail.bottom
            })
            .max(0.0)
                * scale,
        }
    }
}
pub fn default_rect(work: Rect, dpi: u32) -> Rect {
    Placement {
        monitor: String::new(),
        anchor: Anchor::BottomRight,
        x_dip: 16.0,
        y_dip: 12.0,
    }
    .rect(work, dpi)
}
pub fn snap(rail: Rect, work: Rect, dpi: u32) -> Rect {
    let gap = px(8.0, dpi);
    let range = px(20.0, dpi);
    let mut result = rail.clamp(work);
    let width = result.width();
    let height = result.height();
    if result.left - work.left < range {
        result.left = work.left + gap;
    } else if work.right - result.right < range {
        result.left = work.right - width - gap;
    }
    if result.top - work.top < range {
        result.top = work.top + gap;
    } else if work.bottom - result.bottom < range {
        result.top = work.bottom - height - gap;
    }
    result.right = result.left + width;
    result.bottom = result.top + height;
    result.clamp(work)
}
pub fn card_rect(rail: Rect, work: Rect, dpi: u32) -> Rect {
    let width = px(CARD_WIDTH, dpi);
    let height = px(CARD_HEIGHT, dpi);
    let gap = px(8.0, dpi);
    let top = if rail.top - work.top >= height + gap {
        rail.top - height - gap
    } else {
        rail.bottom + gap
    };
    Rect {
        left: rail.right - width,
        top,
        right: rail.right,
        bottom: top + height,
    }
    .clamp(work)
}

/// The rail may cover the taskbar; the popup remains constrained to work area.
pub fn snap_over_taskbar(rail: Rect, screen: Rect, work: Rect, dpi: u32) -> Rect {
    let mut r = snap(rail, screen, dpi);
    let cy = (i64::from(rail.top) + i64::from(rail.bottom)) / 2;
    let cx = (i64::from(rail.left) + i64::from(rail.right)) / 2;
    let width = r.width();
    let height = r.height();
    if work.bottom < screen.bottom && cy >= i64::from(work.bottom) {
        r.top = work.bottom + (screen.bottom - work.bottom - height) / 2;
        r.bottom = r.top + height;
    } else if work.top > screen.top && cy < i64::from(work.top) {
        r.top = screen.top + (work.top - screen.top - height) / 2;
        r.bottom = r.top + height;
    } else if work.left > screen.left && cx < i64::from(work.left) {
        r.left = screen.left + (work.left - screen.left - width) / 2;
        r.right = r.left + width;
    } else if work.right < screen.right && cx >= i64::from(work.right) {
        r.left = work.right + (screen.right - work.right - width) / 2;
        r.right = r.left + width;
    }
    r.clamp(screen)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WidgetConfig {
    pub schema_version: u32,
    pub visible: bool,
    pub locked: bool,
    pub theme: Theme,
    pub placement: Option<Placement>,
    #[serde(default)]
    pub taskbar_offset_dip: f64,
}
impl Default for WidgetConfig {
    fn default() -> Self {
        Self {
            schema_version: 1,
            visible: true,
            locked: false,
            theme: Theme::System,
            placement: None,
            taskbar_offset_dip: 0.0,
        }
    }
}
impl WidgetConfig {
    pub fn load(root: &Path) -> std::io::Result<Self> {
        let file = match File::open(root.join("widget.json")) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(e),
        };
        let mut bytes = Vec::new();
        file.take(16_385).read_to_end(&mut bytes)?;
        let invalid = || {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Invalid QuotaRail widget settings.",
            )
        };
        if bytes.len() > 16_384 {
            return Err(invalid());
        }
        let config: Self = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
        if config.schema_version != 1
            || !config.taskbar_offset_dip.is_finite()
            || !(0.0..=1_000_000.0).contains(&config.taskbar_offset_dip)
            || config.placement.as_ref().is_some_and(|p| {
                p.monitor.len() > 128
                    || !p.x_dip.is_finite()
                    || !p.y_dip.is_finite()
                    || !(0.0..=1_000_000.0).contains(&p.x_dip)
                    || !(0.0..=1_000_000.0).contains(&p.y_dip)
            })
        {
            return Err(invalid());
        }
        Ok(config)
    }
    pub fn save(&self, root: &Path) -> std::io::Result<()> {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::{
            MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
        };
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let temporary = root.join(format!("widget-{}-{nonce}.tmp", std::process::id()));
        let bytes = serde_json::to_vec(self)?;
        let result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            drop(file);
            let src: Vec<u16> = temporary.as_os_str().encode_wide().chain(Some(0)).collect();
            let dst: Vec<u16> = root
                .join("widget.json")
                .as_os_str()
                .encode_wide()
                .chain(Some(0))
                .collect();
            if unsafe {
                MoveFileExW(
                    src.as_ptr(),
                    dst.as_ptr(),
                    MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
                )
            } == 0
            {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(temporary);
        }
        result
    }
}
