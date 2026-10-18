use crate::{
    domain::QuotaSnapshot,
    error::{ErrorKind, ProbeError},
};

#[derive(Clone, Default)]
pub struct DesktopState {
    pub generation: u64,
    pub loading: bool,
    pub snapshot: Option<QuotaSnapshot>,
    pub history: Option<QuotaSnapshot>,
    pub error: Option<ProbeError>,
    pub cache_warning: bool,
    pub manual_available_at: u64,
    pub refresh_minutes: u32,
}

impl DesktopState {
    pub fn reconnect(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.snapshot = None;
        self.error = None;
    }

    pub fn apply(
        &mut self,
        generation: u64,
        result: std::result::Result<QuotaSnapshot, ProbeError>,
    ) -> bool {
        if generation != self.generation {
            return false;
        }
        self.loading = false;
        match result {
            Ok(snapshot) => {
                if self
                    .snapshot
                    .as_ref()
                    .and_then(|old| old.account.same_backend_identity(&snapshot.account))
                    == Some(false)
                {
                    self.generation = self.generation.wrapping_add(1);
                    self.history = None;
                }
                self.history = Some(snapshot.clone());
                self.snapshot = Some(snapshot);
                self.error = None;
            }
            Err(error) => {
                if auth_requires_action(error.kind) {
                    self.snapshot = None;
                }
                self.error = Some(error);
            }
        }
        true
    }
}

pub fn auth_requires_action(kind: ErrorKind) -> bool {
    matches!(
        kind,
        ErrorKind::SignedOut | ErrorKind::AuthenticationRejected | ErrorKind::UnsupportedAuth
    )
}

pub fn retry_delay(failures: u32, jitter: u64) -> u64 {
    let base = match failures {
        0 | 1 => 300,
        2 => 600,
        3 => 1200,
        _ => 1800,
    };
    base + jitter % 31
}

pub fn manual_allowed(now: u64, state: &DesktopState) -> bool {
    !state.loading && now >= state.manual_available_at
}
