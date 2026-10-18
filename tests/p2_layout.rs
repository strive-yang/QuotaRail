mod p1_common;
use p1_common::Temp;
use quotarail::desktop::widget_layout::*;

#[test]
fn dip_placement_survives_scale_and_negative_monitor_coordinates() {
    let work = Rect {
        left: -2560,
        top: -200,
        right: 0,
        bottom: 1200,
    };
    let first = default_rect(work, 144);
    let saved = Placement::capture("SYNTHETIC_MONITOR".into(), first, work, 144);
    assert_eq!(saved.anchor, Anchor::BottomRight);
    assert_eq!(saved.rect(work, 144), first);
    let scaled = saved.rect(work, 192);
    assert_eq!(scaled.width(), 316);
    assert_eq!(work.right - scaled.right, 32);
    assert_eq!(work.bottom - scaled.bottom, 24);
}
#[test]
fn removed_or_shrunken_work_area_never_leaves_widget_offscreen() {
    let work = Rect {
        left: 0,
        top: 0,
        right: 1280,
        bottom: 720,
    };
    let p = Placement {
        monitor: "SYNTHETIC_MISSING".into(),
        anchor: Anchor::TopLeft,
        x_dip: 90000.0,
        y_dip: 90000.0,
    };
    let r = p.rect(work, 96);
    assert!(r.left >= work.left && r.right <= work.right && r.bottom <= work.bottom);
    let tiny = Rect {
        left: 0,
        top: 0,
        right: 150,
        bottom: 35,
    };
    let tiny_rect = p.rect(tiny, 192);
    assert_eq!(tiny_rect, tiny);
}
#[test]
fn popup_flips_and_clamps_and_snap_preserves_visible_edges() {
    let work = Rect {
        left: -1920,
        top: 0,
        right: 0,
        bottom: 1040,
    };
    let top = Rect {
        left: -1900,
        top: 5,
        right: -1692,
        bottom: 57,
    };
    let popup = card_rect(top, work, 96);
    assert!(popup.top >= top.bottom && popup.left >= work.left && popup.right <= work.right);
    let bottom = default_rect(work, 96);
    assert!(card_rect(bottom, work, 96).bottom < bottom.top);
    let snapped = snap(
        Rect {
            left: -1950,
            top: 1000,
            right: -1742,
            bottom: 1052,
        },
        work,
        96,
    );
    assert_eq!(snapped.left, work.left + 8);
    assert_eq!(snapped.bottom, work.bottom - 8);
}
#[test]
fn taskbar_overlay_is_allowed_but_popup_stays_outside_taskbar() {
    let screen = Rect {
        left: 0,
        top: 0,
        right: 1920,
        bottom: 1080,
    };
    let work = Rect {
        bottom: 1032,
        ..screen
    };
    let r = snap_over_taskbar(
        Rect {
            left: 200,
            top: 1032,
            right: 358,
            bottom: 1072,
        },
        screen,
        work,
        96,
    );
    assert_eq!(r.top, 1036);
    assert_eq!(r.bottom, 1076);
    assert!(card_rect(r, work, 96).bottom <= work.bottom);
    let saved = Placement::capture("SYNTHETIC_MONITOR".into(), r, screen, 96);
    assert_eq!(saved.rect(screen, 96), r);
}
#[test]
fn position_theme_lock_and_visibility_roundtrip_and_corruption_is_explicit() {
    let temp = Temp::new();
    let mut config = WidgetConfig::load(&temp.0).unwrap();
    config.locked = true;
    config.visible = false;
    config.theme = Theme::Dark;
    config.placement = Some(Placement {
        monitor: "SYNTHETIC_MONITOR".into(),
        anchor: Anchor::BottomRight,
        x_dip: 12.0,
        y_dip: 8.0,
    });
    config.save(&temp.0).unwrap();
    let loaded = WidgetConfig::load(&temp.0).unwrap();
    assert!(loaded.locked);
    assert!(!loaded.visible);
    assert_eq!(loaded.theme, Theme::Dark);
    assert_eq!(loaded.placement.unwrap().monitor, "SYNTHETIC_MONITOR");
    std::fs::write(temp.0.join("widget.json"), b"{\"schema_version\":999}").unwrap();
    assert!(WidgetConfig::load(&temp.0).is_err());
    std::fs::write(temp.0.join("widget.json"), vec![b'x'; 16385]).unwrap();
    assert!(WidgetConfig::load(&temp.0).is_err());
}

#[test]
fn legacy_widget_settings_gain_an_independent_validated_taskbar_offset() {
    let temp = Temp::new();
    std::fs::write(
        temp.0.join("widget.json"),
        br#"{"schema_version":1,"visible":true,"locked":false,"theme":"system","placement":null}"#,
    )
    .unwrap();
    let mut config = WidgetConfig::load(&temp.0).unwrap();
    assert_eq!(config.taskbar_offset_dip, 0.0);
    config.taskbar_offset_dip = 65.3333333333;
    config.save(&temp.0).unwrap();
    assert_eq!(
        WidgetConfig::load(&temp.0).unwrap().taskbar_offset_dip,
        config.taskbar_offset_dip
    );
    config.taskbar_offset_dip = -1.0;
    config.save(&temp.0).unwrap();
    assert!(WidgetConfig::load(&temp.0).is_err());
}
