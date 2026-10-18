#![allow(dead_code)]
use quotarail::{domain::QuotaSnapshot, wire::RateLimitsResponse};
use serde_json::json;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
static COUNTER: AtomicU64 = AtomicU64::new(0);

pub struct Temp(pub PathBuf);
impl Temp {
    pub fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "quotarail-p1-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        if let (Ok(root), Ok(target)) = (std::env::temp_dir().canonicalize(), self.0.canonicalize())
            && target.parent() == Some(root.as_path())
            && target
                .file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("quotarail-p1-"))
        {
            let _ = std::fs::remove_dir_all(target);
        }
    }
}

pub fn snapshot(account: Option<&str>, percent: f64, fetched: u64) -> QuotaSnapshot {
    serde_json::from_value::<RateLimitsResponse>(json!({"accountId":account,"rateLimits":{"limitId":"codex","primary":{"usedPercent":percent,"windowDurationMins":10080,"resetsAt":null}}})).unwrap().normalize("synthetic".into(),fetched).unwrap()
}
