pub mod autostart;
pub mod preferences;
pub mod settings;
pub mod state;
pub mod taskbar;
pub mod view;
pub mod widget;
pub mod widget_layout;
mod widget_paint;
mod widget_surface;
pub mod windows;
pub mod worker;

use crate::error::{ErrorKind, ProbeError, Result};
use preferences::{DisplayMode, Preferences};
use std::{ffi::OsString, path::PathBuf};

#[derive(Clone)]
pub struct DesktopOptions {
    pub codex: Option<PathBuf>,
    pub home: PathBuf,
    pub data_dir: PathBuf,
    pub tray_only: bool,
    pub taskbar: bool,
    pub taskbar_left_space: u32,
    pub refresh_minutes: u32,
    pub settings_warning: bool,
}

impl DesktopOptions {
    pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Self> {
        let mut args = args.into_iter();
        let mut codex = None;
        let mut home = None;
        let mut data_dir = None;
        let mut tray_only = false;
        let mut taskbar = None;
        let mut taskbar_left_space = None;
        let mut refresh_minutes = None;
        while let Some(arg) = args.next() {
            if arg == "--tray-only" {
                tray_only = true;
                continue;
            }
            if arg == "--taskbar" || arg == "--floating" {
                let requested = arg == "--taskbar";
                if taskbar.is_some_and(|previous| previous != requested) {
                    return Err(arguments_error());
                }
                taskbar = Some(requested);
                continue;
            }
            let value = args.next().ok_or_else(arguments_error)?;
            match arg.to_str() {
                Some("--codex") => codex = Some(PathBuf::from(value)),
                Some("--codex-home") => home = Some(PathBuf::from(value)),
                Some("--data-dir") => data_dir = Some(PathBuf::from(value)),
                Some("--taskbar-left-space") => {
                    taskbar_left_space = Some(
                        value
                            .to_str()
                            .and_then(|s| s.parse::<u32>().ok())
                            .filter(|n| *n <= 1024)
                            .ok_or_else(arguments_error)?,
                    );
                }
                Some("--refresh-minutes") => {
                    refresh_minutes = Some(
                        value
                            .to_str()
                            .and_then(|s| s.parse::<u32>().ok())
                            .filter(|n| (5..=60).contains(n))
                            .ok_or_else(arguments_error)?,
                    )
                }
                _ => return Err(arguments_error()),
            }
        }
        if tray_only && taskbar.is_some() {
            return Err(arguments_error());
        }
        let data_dir = data_dir
            .or_else(|| {
                std::env::var_os("LOCALAPPDATA").map(|p| PathBuf::from(p).join("QuotaRail"))
            })
            .ok_or_else(arguments_error)?;
        if !data_dir.is_absolute() {
            return Err(arguments_error());
        }
        let loaded = Preferences::load(&data_dir);
        let settings_warning = loaded.is_err();
        let saved = loaded.unwrap_or_default();
        let mode = if tray_only {
            DisplayMode::Tray
        } else {
            taskbar
                .map(|enabled| {
                    if enabled {
                        DisplayMode::Taskbar
                    } else {
                        DisplayMode::Floating
                    }
                })
                .unwrap_or(saved.mode)
        };
        let taskbar = mode == DisplayMode::Taskbar;
        let tray_only = mode == DisplayMode::Tray;
        if taskbar_left_space.is_some() && !taskbar {
            return Err(arguments_error());
        }
        let home = home
            .or(saved.codex_home)
            .map(Ok)
            .unwrap_or_else(default_home)?;
        if !home.is_absolute() {
            return Err(arguments_error());
        }
        Ok(Self {
            codex: codex.or(saved.codex),
            home,
            data_dir,
            tray_only,
            taskbar,
            taskbar_left_space: taskbar_left_space.unwrap_or(saved.taskbar_left_space),
            refresh_minutes: refresh_minutes.unwrap_or(saved.refresh_minutes),
            settings_warning,
        })
    }
    pub fn preferences(&self) -> Preferences {
        Preferences {
            mode: if self.tray_only {
                DisplayMode::Tray
            } else if self.taskbar {
                DisplayMode::Taskbar
            } else {
                DisplayMode::Floating
            },
            codex: self.codex.clone(),
            codex_home: Some(self.home.clone()),
            refresh_minutes: self.refresh_minutes,
            taskbar_left_space: self.taskbar_left_space,
            ..Default::default()
        }
    }
    pub fn updated(&self, preferences: &Preferences) -> Result<Self> {
        preferences.validate().map_err(|_| arguments_error())?;
        Ok(Self {
            codex: preferences.codex.clone(),
            home: preferences
                .codex_home
                .clone()
                .map(Ok)
                .unwrap_or_else(default_home)?,
            data_dir: self.data_dir.clone(),
            taskbar: preferences.mode == DisplayMode::Taskbar,
            tray_only: preferences.mode == DisplayMode::Tray,
            taskbar_left_space: preferences.taskbar_left_space,
            refresh_minutes: preferences.refresh_minutes,
            settings_warning: false,
        })
    }
}

fn default_home() -> Result<PathBuf> {
    std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(|p| PathBuf::from(p).join(".codex")))
        .filter(|p| p.is_absolute())
        .ok_or_else(arguments_error)
}

fn arguments_error() -> ProbeError {
    ProbeError::new(
        ErrorKind::InvalidArguments,
        "Use --codex PATH, --codex-home ABSOLUTE_PATH, --data-dir ABSOLUTE_PATH, --taskbar-left-space DIP, --refresh-minutes 5..60, or --taskbar / --floating / --tray-only. Explicit modes cannot be combined; command-line options override saved settings.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn taskbar_is_default_with_explicit_floating_and_tray_fallbacks() {
        let parse = |flags: &[&str]| {
            DesktopOptions::parse(
                [
                    "--codex-home",
                    "C:\\SYNTHETIC_HOME",
                    "--data-dir",
                    "C:\\SYNTHETIC_DATA",
                ]
                .into_iter()
                .chain(flags.iter().copied())
                .map(OsString::from),
            )
        };
        let default = parse(&[]).unwrap();
        assert!(default.taskbar && !default.tray_only);
        let floating = parse(&["--floating"]).unwrap();
        assert!(!floating.taskbar && !floating.tray_only);
        let tray = parse(&["--tray-only"]).unwrap();
        assert!(!tray.taskbar && tray.tray_only);
        let options = parse(&["--taskbar"]).unwrap();
        assert!(options.taskbar);
        assert_eq!(options.taskbar_left_space, 240);
        assert_eq!(
            parse(&["--taskbar-left-space", "320"])
                .unwrap()
                .taskbar_left_space,
            320
        );
        for flags in [
            vec!["--taskbar", "--tray-only"],
            vec!["--tray-only", "--taskbar"],
            vec!["--floating", "--tray-only"],
            vec!["--tray-only", "--floating"],
            vec!["--taskbar", "--floating"],
            vec!["--floating", "--taskbar"],
            vec!["--floating", "--taskbar-left-space", "240"],
            vec!["--tray-only", "--taskbar-left-space", "240"],
            vec!["--taskbar", "--taskbar-left-space", "1025"],
            vec!["--taskbar", "--taskbar-left-space", "-1"],
            vec!["--taskbar", "--taskbar-left-space", "invalid"],
        ] {
            assert!(parse(&flags).is_err());
        }
    }
}
