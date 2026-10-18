mod p1_common;
use p1_common::snapshot;
use quotarail::{
    desktop::{
        state::{DesktopState, manual_allowed, retry_delay},
        view,
    },
    error::{ErrorKind, ProbeError},
};

#[test]
fn failed_read_preserves_last_success_but_never_turns_it_into_zero() {
    let mut state = DesktopState::default();
    assert!(state.apply(0, Ok(snapshot(Some("SYNTHETIC_A"), 31.25, 1000))));
    state.loading = true;
    assert!(view::tooltip(&state, 1001).contains("刷新中"));
    assert!(state.apply(0, Err(ProbeError::new(ErrorKind::RequestFailed, "safe"))));
    assert_eq!(
        state.snapshot.as_ref().unwrap().fetched_at_unix_seconds,
        1000
    );
    let text = view::tooltip(&state, 1020);
    assert!(text.contains("刷新失败"));
    assert!(text.contains("31.2%"));
    assert!(text.contains("5H —"));
    assert!(!text.contains("5H 已用 0"));
}

#[test]
fn reconnect_rejects_late_results_and_account_changes_clear_old_data() {
    let mut state = DesktopState::default();
    state.apply(0, Ok(snapshot(Some("SYNTHETIC_A"), 10.0, 1000)));
    state.reconnect();
    assert!(!state.apply(0, Ok(snapshot(Some("SYNTHETIC_A"), 80.0, 1100))));
    assert!(state.snapshot.is_none());
    state.apply(1, Ok(snapshot(Some("SYNTHETIC_B"), 20.0, 1200)));
    assert_eq!(
        state.snapshot.as_ref().unwrap().buckets[0].windows[0].used_percent,
        20.0
    );
    state.apply(1, Err(ProbeError::new(ErrorKind::SignedOut, "safe")));
    assert!(state.snapshot.is_none());
    assert!(!view::tooltip(&state, 1200).contains("20.0%"));
}

#[test]
fn startup_history_is_explicit_and_not_current_account_data() {
    let state = DesktopState {
        history: Some(snapshot(None, 31.25, 1000)),
        ..Default::default()
    };
    let tooltip = view::tooltip(&state, 2000);
    assert!(!tooltip.contains("31.2%"));
    assert!(tooltip.contains("账户尚未核验"));
    let history = view::historical_details(&state, 2000, |_| "synthetic time".into());
    assert!(history.contains("不代表当前账户额度"));
    assert!(history.contains("31.2%"));
}

#[test]
fn cooldown_backoff_and_staleness_are_bounded() {
    let mut state = DesktopState {
        manual_available_at: 1030,
        ..Default::default()
    };
    assert!(!manual_allowed(1029, &state));
    assert!(manual_allowed(1030, &state));
    state.loading = true;
    assert!(!manual_allowed(2000, &state));
    assert_eq!(retry_delay(1, 0), 300);
    assert_eq!(retry_delay(2, 0), 600);
    assert_eq!(retry_delay(3, 0), 1200);
    assert_eq!(retry_delay(u32::MAX, 30), 1830);
    state.apply(0, Ok(snapshot(Some("SYNTHETIC_A"), 50.0, 1000)));
    assert_eq!(view::status(&state, 1900), "缓存过期");
    assert!(view::details(&state, 1900, |_| "time".into()).contains("50.0%"));
}

#[test]
fn backend_blocks_and_invalid_percent_are_not_reported_as_healthy() {
    let mut state = DesktopState::default();
    let mut blocked = snapshot(Some("SYNTHETIC_A"), 30.0, 1000);
    blocked.ordinary_usage_allowed = Some(false);
    state.apply(0, Ok(blocked));
    assert_eq!(view::status(&state, 1001), "服务端用量受限");
    state.apply(0, Ok(snapshot(Some("SYNTHETIC_A"), 120.0, 1000)));
    assert_eq!(view::status(&state, 1001), "额度比例异常");
}

#[test]
fn primary_ui_percent_and_bar_show_remaining_with_missing_and_invalid_kept_unknown() {
    for (used, remaining) in [(0.0, 100.0), (42.0, 58.0), (68.0, 32.0), (100.0, 0.0)] {
        let snapshot = snapshot(Some("SYNTHETIC_A"), used, 1000);
        let bucket = &snapshot.buckets[0];
        assert_eq!(view::remaining_percent(&bucket.windows[0]), Some(remaining));
        assert!(view::window_line(bucket, 10080).starts_with(&format!("7D 剩余 {remaining:.1}%")));
        assert!(view::window_line(bucket, 300).contains("本次未提供"));
    }
    for used in [-1.0, 120.0] {
        let snapshot = snapshot(Some("SYNTHETIC_A"), used, 1000);
        assert_eq!(
            view::remaining_percent(&snapshot.buckets[0].windows[0]),
            None
        );
        assert!(view::window_values(&snapshot.buckets[0].windows[0]).starts_with("剩余未知"));
    }
}
