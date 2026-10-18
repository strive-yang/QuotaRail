use quotarail::{domain::WindowAvailability, error::ErrorKind, wire::RateLimitsResponse};
use serde_json::{Value, json};

fn normalize(value: Value) -> quotarail::error::Result<quotarail::domain::QuotaSnapshot> {
    serde_json::from_value::<RateLimitsResponse>(value)
        .unwrap()
        .normalize("synthetic".into(), 100)
}

fn window(minutes: Value, percent: f64) -> Value {
    json!({"windowDurationMins":minutes,"usedPercent":percent,"resetsAt":null})
}

#[test]
fn weekly_primary_is_not_mislabeled_five_hour() {
    let result = normalize(json!({"rateLimits":{"primary":window(json!(10080),31.25)}})).unwrap();
    let bucket = &result.buckets[0];
    assert_eq!(bucket.five_hour, WindowAvailability::NotProvided);
    assert_eq!(bucket.seven_day, WindowAvailability::Provided);
    assert_eq!(bucket.windows[0].label, "7D");
    assert_eq!(bucket.windows[0].remaining_percent, 68.75);
}

#[test]
fn reversed_roles_and_missing_reset_are_preserved() {
    let result = normalize(json!({"rateLimits":{"primary":window(json!(10080),1.0),"secondary":window(json!(300),2.0)}})).unwrap();
    assert_eq!(result.buckets[0].windows[1].label, "5H");
    assert!(
        result.buckets[0].windows[1]
            .resets_at_unix_seconds
            .is_none()
    );
}

#[test]
fn unknown_duration_is_not_proof_of_missing_window() {
    let result = normalize(json!({"rateLimits":{"primary":window(Value::Null,22.0)}})).unwrap();
    assert_eq!(
        result.buckets[0].five_hour,
        WindowAvailability::UnidentifiedDuration
    );
    assert_eq!(
        result.buckets[0].seven_day,
        WindowAvailability::UnidentifiedDuration
    );
}

#[test]
fn empty_windows_are_not_zero_usage() {
    let result = normalize(json!({"rateLimits":{}})).unwrap();
    assert!(result.buckets[0].windows.is_empty());
    assert_eq!(result.buckets[0].seven_day, WindowAvailability::NotProvided);
}

#[test]
fn map_wins_without_merging_legacy_bucket() {
    let result = normalize(json!({"rateLimits":{"primary":window(json!(300),99.0)},"rateLimitsByLimitId":{"codex_other":{"limitId":"codex_other","primary":window(json!(60),2.0)}}})).unwrap();
    assert_eq!(result.buckets.len(), 1);
    assert_eq!(result.buckets[0].id, "codex_other");
    assert_eq!(result.buckets[0].windows[0].label, "60m");
}

#[test]
fn null_map_falls_back_but_empty_map_is_an_error() {
    assert!(normalize(json!({"rateLimits":{},"rateLimitsByLimitId":null})).is_ok());
    assert_eq!(
        normalize(json!({"rateLimits":{},"rateLimitsByLimitId":{}}))
            .unwrap_err()
            .kind,
        ErrorKind::Protocol
    );
}

#[test]
fn multiple_buckets_stay_separate_and_identity_is_not_serialized() {
    let result = normalize(json!({"accountId":"SYNTHETIC_SECRET_ID","rateLimitsByLimitId":{"codex":{"primary":window(json!(300),20.0)},"other":{"primary":window(json!(300),40.0)}}})).unwrap();
    assert_eq!(result.buckets.len(), 2);
    assert!(!format!("{result:?}").contains("SYNTHETIC_SECRET_ID"));
    assert!(
        !serde_json::to_string(&result)
            .unwrap()
            .contains("SYNTHETIC_SECRET_ID")
    );
}

#[test]
fn out_of_range_percent_is_flagged_not_silently_fixed() {
    let result = normalize(json!({"rateLimits":{"primary":window(json!(300),120.0)}})).unwrap();
    let value = &result.buckets[0].windows[0];
    assert_eq!(value.used_percent, 120.0);
    assert_eq!(value.remaining_percent, 0.0);
    assert_eq!(value.bar_percent, 100.0);
    assert!(value.percent_out_of_range);
}

#[test]
fn invalid_duration_and_conflicting_id_are_rejected() {
    assert!(normalize(json!({"rateLimits":{"primary":window(json!(-1),1.0)}})).is_err());
    assert!(normalize(json!({"rateLimitsByLimitId":{"codex":{"limitId":"other"}}})).is_err());
    assert!(normalize(json!({})).is_err());
}

#[test]
fn past_reset_does_not_reset_usage() {
    let result = normalize(json!({"rateLimits":{"primary":{"usedPercent":88.0,"windowDurationMins":300,"resetsAt":1}}})).unwrap();
    assert_eq!(result.buckets[0].windows[0].used_percent, 88.0);
}
