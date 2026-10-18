use crate::domain::{
    Account, AuthType, DataSource, DataStatus, LimitBucket, LimitWindow, QuotaSnapshot,
    WindowAvailability,
};
use crate::error::{ErrorKind, ProbeError, Result};
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RateLimitsResponse {
    pub rate_limits: Option<WireBucket>,
    pub rate_limits_by_limit_id: Option<BTreeMap<String, WireBucket>>,
    pub account_id: Option<String>,
    pub ordinary_usage_allowed: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireBucket {
    pub limit_id: Option<String>,
    pub primary: Option<WireWindow>,
    pub secondary: Option<WireWindow>,
    pub spend_control_reached: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireWindow {
    pub used_percent: f64,
    pub window_duration_mins: Option<i64>,
    pub resets_at: Option<i64>,
}

fn protocol_error() -> ProbeError {
    ProbeError::new(
        ErrorKind::Protocol,
        "Quota response has an invalid structure.",
    )
}

impl WireWindow {
    fn normalize(self, role: &'static str) -> Result<LimitWindow> {
        if !self.used_percent.is_finite()
            || self.window_duration_mins.is_some_and(|v| v <= 0)
            || self.resets_at.is_some_and(|v| v < 0)
        {
            return Err(protocol_error());
        }
        let label = match self.window_duration_mins {
            Some(300) => "5H".to_owned(),
            Some(10080) => "7D".to_owned(),
            Some(minutes) => format!("{minutes}m"),
            None => "unknown".to_owned(),
        };
        Ok(LimitWindow {
            source_role: role,
            label,
            used_percent: self.used_percent,
            remaining_percent: (100.0 - self.used_percent).clamp(0.0, 100.0),
            bar_percent: self.used_percent.clamp(0.0, 100.0),
            percent_out_of_range: !(0.0..=100.0).contains(&self.used_percent),
            window_duration_minutes: self.window_duration_mins,
            resets_at_unix_seconds: self.resets_at,
        })
    }
}

fn availability(windows: &[LimitWindow], minutes: i64) -> WindowAvailability {
    if windows
        .iter()
        .any(|w| w.window_duration_minutes == Some(minutes))
    {
        WindowAvailability::Provided
    } else if windows.iter().any(|w| w.window_duration_minutes.is_none()) {
        WindowAvailability::UnidentifiedDuration
    } else {
        WindowAvailability::NotProvided
    }
}

impl RateLimitsResponse {
    pub fn normalize(self, cli_version: String, fetched_at: u64) -> Result<QuotaSnapshot> {
        let wire_buckets = match self.rate_limits_by_limit_id {
            Some(map) if !map.is_empty() => map,
            Some(_) => return Err(protocol_error()),
            None => {
                let bucket = self.rate_limits.ok_or_else(protocol_error)?;
                let key = bucket
                    .limit_id
                    .clone()
                    .unwrap_or_else(|| "codex".to_owned());
                BTreeMap::from([(key, bucket)])
            }
        };
        let mut buckets = Vec::with_capacity(wire_buckets.len());
        for (id, bucket) in wire_buckets {
            if id.is_empty() || bucket.limit_id.as_ref().is_some_and(|v| v != &id) {
                return Err(protocol_error());
            }
            let mut windows = Vec::with_capacity(2);
            if let Some(primary) = bucket.primary {
                windows.push(primary.normalize("primary")?);
            }
            if let Some(secondary) = bucket.secondary {
                windows.push(secondary.normalize("secondary")?);
            }
            buckets.push(LimitBucket {
                five_hour: availability(&windows, 300),
                seven_day: availability(&windows, 10080),
                id,
                windows,
                spend_control_reached: bucket.spend_control_reached,
            });
        }
        Ok(QuotaSnapshot {
            data_status: DataStatus::successful_read(),
            provider: "codex",
            account: Account {
                auth_type: AuthType::Chatgpt,
                identity_status: if self.account_id.is_some() {
                    "backend_reported"
                } else {
                    "unknown"
                },
                backend_id: self.account_id,
            },
            source: DataSource {
                kind: "app_server",
                environment: "windows_native",
                cli_version,
            },
            fetched_at_unix_seconds: fetched_at,
            buckets,
            ordinary_usage_allowed: self.ordinary_usage_allowed,
        })
    }
}
