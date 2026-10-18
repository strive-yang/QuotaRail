//! Versioned application preferences. Widget schema/positions remain independent.
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::windows::ffi::OsStrExt,
    path::{Path, PathBuf},
};
use windows_sys::Win32::Storage::FileSystem::{
    MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayMode {
    #[default]
    Taskbar,
    Floating,
    Tray,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Preferences {
    pub schema_version: u32,
    pub mode: DisplayMode,
    pub refresh_minutes: u32,
    pub taskbar_left_space: u32,
    pub codex: Option<PathBuf>,
    pub codex_home: Option<PathBuf>,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            schema_version: 1,
            mode: DisplayMode::Taskbar,
            refresh_minutes: 5,
            taskbar_left_space: 240,
            codex: None,
            codex_home: None,
        }
    }
}
fn invalid() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        "Invalid QuotaRail application settings.",
    )
}
impl Preferences {
    pub fn validate(&self) -> std::io::Result<()> {
        if self.schema_version != 1
            || !(5..=60).contains(&self.refresh_minutes)
            || self.taskbar_left_space > 1024
            || [&self.codex, &self.codex_home]
                .into_iter()
                .flatten()
                .any(|p| {
                    !p.is_absolute()
                        || p.to_str()
                            .is_none_or(|s| s.len() > 4096 || s.contains(['\0', '"', '\r', '\n']))
                })
        {
            return Err(invalid());
        }
        Ok(())
    }
    pub fn load(root: &Path) -> std::io::Result<Self> {
        let file = match File::open(root.join("settings.json")) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(e),
        };
        let mut bytes = Vec::new();
        file.take(32769).read_to_end(&mut bytes)?;
        if bytes.len() > 32768 {
            return Err(invalid());
        }
        let result: Self = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
        result.validate()?;
        Ok(result)
    }
    pub fn save(&self, root: &Path) -> std::io::Result<()> {
        self.validate()?;
        fs::create_dir_all(root)?;
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let temp = root.join(format!("settings-{}-{nonce}.tmp", std::process::id()));
        let result = (|| {
            let bytes = serde_json::to_vec_pretty(self)?;
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            drop(file);
            let src: Vec<u16> = temp.as_os_str().encode_wide().chain(Some(0)).collect();
            let dst: Vec<u16> = root
                .join("settings.json")
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
            let _ = fs::remove_file(temp);
        }
        result
    }
}
