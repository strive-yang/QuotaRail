mod p1_common;
use p1_common::{Temp, snapshot};
use quotarail::{
    domain::{AuthState, Freshness},
    storage::CacheStore,
};

#[test]
fn cache_roundtrip_is_redacted_source_scoped_and_identity_checked() {
    let dir = Temp::new();
    let store = CacheStore::new(&dir.0, &dir.0.join("cli.exe"), &dir.0.join("home")).unwrap();
    let original = snapshot(Some("SYNTHETIC_PRIVATE_ACCOUNT"), 31.25, 1000);
    assert!(store.save(&original).unwrap());
    let raw = std::fs::read_to_string(store.file_path()).unwrap();
    assert!(!raw.contains("SYNTHETIC_PRIVATE_ACCOUNT"));
    assert!(!raw.contains("access_token"));
    let cached = store.load(2000).unwrap().unwrap();
    assert_eq!(cached.snapshot.fetched_at_unix_seconds, 1000);
    assert_eq!(cached.snapshot.data_status.auth, AuthState::Unknown);
    assert_eq!(cached.snapshot.data_status.freshness, Freshness::Stale);
    assert!(store.matches(&cached, &original.account));
    assert!(!store.matches(&cached, &snapshot(Some("OTHER_ACCOUNT"), 0.0, 1000).account));
    assert!(!store.matches(&cached, &snapshot(None, 0.0, 1000).account));
    let same = CacheStore::new(&dir.0, &dir.0.join("cli.exe"), &dir.0.join("home")).unwrap();
    assert!(same.matches(&same.load(2000).unwrap().unwrap(), &original.account));
    let other = CacheStore::new(&dir.0, &dir.0.join("other-cli.exe"), &dir.0.join("home")).unwrap();
    assert!(other.load(2000).unwrap().is_none());
}

#[test]
fn corrupt_unknown_version_and_future_cache_are_rejected() {
    let dir = Temp::new();
    let store = CacheStore::new(&dir.0, &dir.0.join("cli.exe"), &dir.0.join("home")).unwrap();
    assert!(store.load(1000).unwrap().is_none());
    std::fs::write(store.file_path(), b"not json").unwrap();
    assert!(store.load(1000).is_err());
    store
        .save(&snapshot(Some("SYNTHETIC_A"), 1.0, 1000))
        .unwrap();
    let mut body: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(store.file_path()).unwrap()).unwrap();
    body["schema_version"] = serde_json::json!(999);
    std::fs::write(store.file_path(), serde_json::to_vec(&body).unwrap()).unwrap();
    assert!(store.load(1000).is_err());
    store
        .save(&snapshot(Some("SYNTHETIC_A"), 1.0, 2000))
        .unwrap();
    assert!(store.load(1000).is_err());
}

#[test]
fn anonymous_or_oversized_writes_do_not_destroy_last_success() {
    let dir = Temp::new();
    let store = CacheStore::new(&dir.0, &dir.0.join("cli.exe"), &dir.0.join("home")).unwrap();
    let original = snapshot(Some("SYNTHETIC_A"), 12.0, 1000);
    store.save(&original).unwrap();
    let before = std::fs::read(store.file_path()).unwrap();
    assert!(!store.save(&snapshot(None, 15.0, 1000)).unwrap());
    let mut huge = original;
    huge.buckets[0].id = "x".repeat(1024 * 1024 + 1);
    assert!(store.save(&huge).is_err());
    assert_eq!(std::fs::read(store.file_path()).unwrap(), before);
}
