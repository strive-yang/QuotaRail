mod p1_common;
use p1_common::{Temp, snapshot};
use quotarail::desktop::{
    DesktopOptions,
    preferences::{DisplayMode, Preferences},
    state::DesktopState,
    view,
    widget_layout::{Theme, WidgetConfig},
};
use std::ffi::OsString;

fn parse(root: &std::path::Path, flags: &[&str]) -> DesktopOptions {
    DesktopOptions::parse(
        [OsString::from("--data-dir"), root.as_os_str().to_owned()]
            .into_iter()
            .chain(flags.iter().map(OsString::from)),
    )
    .unwrap()
}
#[test]
fn defaults_and_partial_schema_preserve_legacy_widget_preferences() {
    let temp = Temp::new();
    assert_eq!(Preferences::load(&temp.0).unwrap(), Preferences::default());
    assert!(!temp.0.join("settings.json").exists());
    let widget = WidgetConfig {
        locked: true,
        theme: Theme::Dark,
        taskbar_offset_dip: 42.0,
        ..Default::default()
    };
    widget.save(&temp.0).unwrap();
    let before = std::fs::read(temp.0.join("widget.json")).unwrap();
    std::fs::write(
        temp.0.join("settings.json"),
        br#"{"schema_version":1,"refresh_minutes":10}"#,
    )
    .unwrap();
    let p = Preferences::load(&temp.0).unwrap();
    assert_eq!(p.refresh_minutes, 10);
    assert_eq!(p.mode, DisplayMode::Taskbar);
    p.save(&temp.0).unwrap();
    assert_eq!(Preferences::load(&temp.0).unwrap(), p);
    assert_eq!(std::fs::read(temp.0.join("widget.json")).unwrap(), before);
}
#[test]
fn explicit_options_override_saved_values_without_rewriting_settings() {
    let temp = Temp::new();
    let p = Preferences {
        mode: DisplayMode::Floating,
        refresh_minutes: 15,
        codex_home: Some(temp.0.clone()),
        ..Default::default()
    };
    p.save(&temp.0).unwrap();
    let before = std::fs::read(temp.0.join("settings.json")).unwrap();
    let saved = parse(&temp.0, &[]);
    assert!(!saved.taskbar && !saved.tray_only);
    assert_eq!(saved.refresh_minutes, 15);
    let explicit = parse(
        &temp.0,
        &[
            "--taskbar",
            "--refresh-minutes",
            "5",
            "--taskbar-left-space",
            "320",
        ],
    );
    assert!(explicit.taskbar);
    assert_eq!(explicit.refresh_minutes, 5);
    assert_eq!(explicit.taskbar_left_space, 320);
    assert_eq!(std::fs::read(temp.0.join("settings.json")).unwrap(), before);
}
#[test]
fn invalid_future_and_oversized_settings_are_preserved_until_explicit_recovery() {
    let temp = Temp::new();
    for bytes in [
        b"not json".to_vec(),
        br#"{"schema_version":99}"#.to_vec(),
        vec![b'x'; 32769],
    ] {
        std::fs::write(temp.0.join("settings.json"), &bytes).unwrap();
        assert!(Preferences::load(&temp.0).is_err());
        let fallback = parse(&temp.0, &[]);
        assert!(fallback.settings_warning && fallback.taskbar);
        assert_eq!(fallback.refresh_minutes, 5);
        assert_eq!(std::fs::read(temp.0.join("settings.json")).unwrap(), bytes);
    }
    Preferences::default().save(&temp.0).unwrap();
    let before = std::fs::read(temp.0.join("settings.json")).unwrap();
    for p in [
        Preferences {
            refresh_minutes: 4,
            ..Default::default()
        },
        Preferences {
            refresh_minutes: 61,
            ..Default::default()
        },
        Preferences {
            taskbar_left_space: 1025,
            ..Default::default()
        },
        Preferences {
            codex: Some("relative.exe".into()),
            ..Default::default()
        },
    ] {
        assert!(p.save(&temp.0).is_err());
        assert_eq!(std::fs::read(temp.0.join("settings.json")).unwrap(), before);
    }
}
#[test]
fn cache_freshness_tracks_selected_interval_without_resetting_quota() {
    let state = DesktopState {
        refresh_minutes: 10,
        snapshot: Some(snapshot(Some("SYNTHETIC_ACCOUNT"), 31.0, 1000)),
        ..Default::default()
    };
    assert_eq!(view::status(&state, 2799), "正常");
    assert_eq!(view::status(&state, 2800), "缓存过期");
    assert_eq!(
        state.snapshot.as_ref().unwrap().buckets[0].windows[0].used_percent,
        31.0
    );
}
