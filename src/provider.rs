use crate::{
    domain::QuotaSnapshot,
    error::{ErrorKind, ProbeError, Result},
    protocol::AppServer,
    wire::RateLimitsResponse,
};
use serde::Deserialize;
use serde_json::json;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Deserialize)]
struct AccountResponse {
    account: Option<AccountWire>,
}
#[derive(Deserialize)]
struct AccountWire {
    #[serde(rename = "type")]
    kind: String,
}

pub async fn read_snapshot(server: &mut AppServer, version: &str) -> Result<QuotaSnapshot> {
    let account = server
        .request(
            "account/read",
            json!({"refreshToken":false}),
            Duration::from_secs(15),
        )
        .await?;
    let account: AccountResponse = serde_json::from_value(account).map_err(|_| {
        ProbeError::new(
            ErrorKind::Protocol,
            "Invalid account response; raw content suppressed.",
        )
    })?;
    let account = account.account.ok_or_else(|| {
        ProbeError::new(
            ErrorKind::SignedOut,
            "Sign in using the Windows-native Codex CLI first.",
        )
    })?;
    if account.kind != "chatgpt" {
        return Err(ProbeError::new(
            ErrorKind::UnsupportedAuth,
            "Only Codex-managed ChatGPT login is supported in P0.",
        ));
    }
    let result = match server
        .request(
            "account/rateLimits/read",
            json!({"excludeResetCreditDetails":true}),
            Duration::from_secs(15),
        )
        .await
    {
        Err(error) if error.kind == ErrorKind::AuthenticationRejected => {
            // A bounded recovery delegated to Codex, never a local OAuth implementation.
            let refreshed = server
                .request(
                    "account/read",
                    json!({"refreshToken":true}),
                    Duration::from_secs(15),
                )
                .await?;
            let refreshed: AccountResponse = serde_json::from_value(refreshed).map_err(|_| {
                ProbeError::new(ErrorKind::Protocol, "Invalid refreshed account response.")
            })?;
            if refreshed
                .account
                .as_ref()
                .is_none_or(|a| a.kind != "chatgpt")
            {
                return Err(ProbeError::new(
                    ErrorKind::AuthenticationRejected,
                    "Codex login is no longer available.",
                ));
            }
            server
                .request(
                    "account/rateLimits/read",
                    json!({"excludeResetCreditDetails":true}),
                    Duration::from_secs(15),
                )
                .await?
        }
        result => result?,
    };
    let wire: RateLimitsResponse = serde_json::from_value(result).map_err(|_| {
        ProbeError::new(
            ErrorKind::Protocol,
            "Invalid quota response; raw content suppressed.",
        )
    })?;
    let fetched = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| ProbeError::new(ErrorKind::Io, "System clock precedes the Unix epoch."))?
        .as_secs();
    wire.normalize(version.to_owned(), fetched)
}
