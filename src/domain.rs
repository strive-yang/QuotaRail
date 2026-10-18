use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthType {
    Chatgpt,
}

#[derive(Clone, Serialize)]
pub struct Account {
    pub auth_type: AuthType,
    pub identity_status: &'static str,
    // Kept in memory for subsequent reads; never included in public output.
    #[serde(skip)]
    pub(crate) backend_id: Option<String>,
}

impl std::fmt::Debug for Account {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Account")
            .field("auth_type", &self.auth_type)
            .field("identity_status", &self.identity_status)
            .finish_non_exhaustive()
    }
}

impl Account {
    pub fn same_backend_identity(&self, other: &Self) -> Option<bool> {
        self.backend_id
            .as_ref()
            .zip(other.backend_id.as_ref())
            .map(|(a, b)| a == b)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DataSource {
    pub kind: &'static str,
    pub environment: &'static str,
    pub cli_version: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct QuotaSnapshot {
    pub data_status: DataStatus,
    pub provider: &'static str,
    pub account: Account,
    pub source: DataSource,
    pub fetched_at_unix_seconds: u64,
    pub buckets: Vec<LimitBucket>,
    pub ordinary_usage_allowed: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct DataStatus {
    pub fetch: FetchState,
    pub auth: AuthState,
    pub freshness: Freshness,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FetchState {
    Idle,
    Loading,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthState {
    Unknown,
    VerifiedForRead,
    SignedOut,
    Rejected,
    Expired,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Freshness {
    NoData,
    Fresh,
    Stale,
}

impl DataStatus {
    pub const fn successful_read() -> Self {
        Self {
            fetch: FetchState::Idle,
            auth: AuthState::VerifiedForRead,
            freshness: Freshness::Fresh,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct LimitBucket {
    pub id: String,
    pub windows: Vec<LimitWindow>,
    pub five_hour: WindowAvailability,
    pub seven_day: WindowAvailability,
    pub spend_control_reached: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowAvailability {
    Provided,
    NotProvided,
    UnidentifiedDuration,
}

#[derive(Debug, Clone, Serialize)]
pub struct LimitWindow {
    pub source_role: &'static str,
    pub label: String,
    pub used_percent: f64,
    pub remaining_percent: f64,
    pub bar_percent: f64,
    pub percent_out_of_range: bool,
    pub window_duration_minutes: Option<i64>,
    pub resets_at_unix_seconds: Option<i64>,
}
