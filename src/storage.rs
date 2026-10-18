//! Only normalized quota fields and salted identity keys are persisted here.
use crate::{
    domain::{Account, AuthState, Freshness, QuotaSnapshot},
    error::{ErrorKind, ProbeError, Result},
    wire::{RateLimitsResponse, WireBucket, WireWindow},
};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::windows::ffi::OsStrExt,
    path::{Path, PathBuf},
    ptr::null_mut,
    time::{SystemTime, UNIX_EPOCH},
};
use windows_sys::Win32::{
    Security::Cryptography::{BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom},
    Storage::FileSystem::{MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW},
};

const MAX_CACHE_BYTES: u64 = 1024 * 1024;

pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub fn path_key(path: &Path) -> String {
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    format!(
        "{:x}",
        Sha256::digest(canonical.to_string_lossy().to_lowercase().as_bytes())
    )
}

fn io_error() -> ProbeError {
    ProbeError::new(ErrorKind::Io, "QuotaRail local cache is unavailable.")
}

#[derive(Serialize, Deserialize)]
struct Envelope {
    schema_version: u32,
    source_key: String,
    account_key: String,
    snapshot: serde_json::Value,
}

pub struct CachedQuota {
    pub snapshot: QuotaSnapshot,
    account_key: String,
}

pub struct CacheStore {
    root: PathBuf,
    source_key: String,
    salt: [u8; 32],
}

impl CacheStore {
    pub fn new(root: &Path, executable: &Path, home: &Path) -> Result<Self> {
        fs::create_dir_all(root).map_err(|_| io_error())?;
        let salt_file = root.join("cache-salt.bin");
        let mut salt = [0u8; 32];
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&salt_file)
        {
            Ok(mut file) => {
                // SAFETY: output is a valid 32-byte buffer, system RNG needs no algorithm handle.
                if unsafe {
                    BCryptGenRandom(
                        null_mut(),
                        salt.as_mut_ptr(),
                        salt.len() as u32,
                        BCRYPT_USE_SYSTEM_PREFERRED_RNG,
                    )
                } != 0
                {
                    let _ = fs::remove_file(&salt_file);
                    return Err(io_error());
                }
                file.write_all(&salt)
                    .and_then(|()| file.sync_all())
                    .map_err(|_| io_error())?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let mut bytes = Vec::new();
                File::open(&salt_file)
                    .and_then(|file| file.take(33).read_to_end(&mut bytes))
                    .map_err(|_| io_error())?;
                if bytes.len() != salt.len() {
                    return Err(io_error());
                }
                salt.copy_from_slice(&bytes);
            }
            Err(_) => return Err(io_error()),
        }
        let source_key = format!(
            "{:x}",
            Sha256::digest(
                format!(
                    "codex-app-server-v1:{}:{}",
                    path_key(executable),
                    path_key(home)
                )
                .as_bytes()
            )
        );
        Ok(Self {
            root: root.to_path_buf(),
            source_key,
            salt,
        })
    }

    pub fn file_path(&self) -> PathBuf {
        self.root.join(format!("quota-{}.json", self.source_key))
    }

    fn account_key(&self, account: &Account) -> Option<String> {
        let id = account.backend_id.as_ref().filter(|id| !id.is_empty())?;
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.salt).ok()?;
        mac.update(b"QuotaRail-account-v1\0");
        mac.update(id.as_bytes());
        Some(format!("{:x}", mac.finalize().into_bytes()))
    }

    pub fn matches(&self, cached: &CachedQuota, account: &Account) -> bool {
        self.account_key(account)
            .is_some_and(|key| key == cached.account_key)
    }

    /// false means the backend supplied no identity; do not write an anonymous cache.
    pub fn save(&self, snapshot: &QuotaSnapshot) -> Result<bool> {
        let Some(account_key) = self.account_key(&snapshot.account) else {
            return Ok(false);
        };
        let envelope = Envelope {
            schema_version: 1,
            source_key: self.source_key.clone(),
            account_key,
            snapshot: serde_json::to_value(snapshot).map_err(|_| io_error())?,
        };
        let bytes = serde_json::to_vec(&envelope).map_err(|_| io_error())?;
        if bytes.len() as u64 > MAX_CACHE_BYTES {
            return Err(io_error());
        }
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let temporary = self.root.join(format!(
            "quota-{}-{}-{nonce}.tmp",
            self.source_key,
            std::process::id()
        ));
        let result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
                .map_err(|_| io_error())?;
            file.write_all(&bytes)
                .and_then(|()| file.sync_all())
                .map_err(|_| io_error())?;
            drop(file);
            let source: Vec<u16> = temporary.as_os_str().encode_wide().chain(Some(0)).collect();
            let destination: Vec<u16> = self
                .file_path()
                .as_os_str()
                .encode_wide()
                .chain(Some(0))
                .collect();
            // SAFETY: both strings are live null-terminated paths to our own cache files.
            if unsafe {
                MoveFileExW(
                    source.as_ptr(),
                    destination.as_ptr(),
                    MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
                )
            } == 0
            {
                return Err(io_error());
            }
            Ok(true)
        })();
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
        result
    }

    pub fn load(&self, now: u64) -> Result<Option<CachedQuota>> {
        let file = match File::open(self.file_path()) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(io_error()),
        };
        let mut bytes = Vec::new();
        file.take(MAX_CACHE_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| io_error())?;
        if bytes.len() as u64 > MAX_CACHE_BYTES {
            return Err(io_error());
        }
        let envelope: Envelope = serde_json::from_slice(&bytes).map_err(|_| io_error())?;
        if envelope.schema_version != 1
            || envelope.source_key != self.source_key
            || envelope.account_key.len() != 64
        {
            return Err(io_error());
        }
        let stored: StoredSnapshot =
            serde_json::from_value(envelope.snapshot).map_err(|_| io_error())?;
        if stored.fetched_at_unix_seconds > now.saturating_add(60) || stored.buckets.is_empty() {
            return Err(io_error());
        }
        let mut map = BTreeMap::new();
        for bucket in stored.buckets {
            let mut wire = WireBucket {
                limit_id: Some(bucket.id.clone()),
                primary: None,
                secondary: None,
                spend_control_reached: bucket.spend_control_reached,
            };
            for window in bucket.windows {
                let slot = match window.source_role.as_str() {
                    "primary" => &mut wire.primary,
                    "secondary" => &mut wire.secondary,
                    _ => return Err(io_error()),
                };
                if slot.is_some() {
                    return Err(io_error());
                }
                *slot = Some(WireWindow {
                    used_percent: window.used_percent,
                    window_duration_mins: window.window_duration_minutes,
                    resets_at: window.resets_at_unix_seconds,
                });
            }
            if map.insert(bucket.id, wire).is_some() {
                return Err(io_error());
            }
        }
        let mut snapshot = RateLimitsResponse {
            rate_limits: None,
            rate_limits_by_limit_id: Some(map),
            account_id: None,
            ordinary_usage_allowed: stored.ordinary_usage_allowed,
        }
        .normalize(stored.source.cli_version, stored.fetched_at_unix_seconds)
        .map_err(|_| io_error())?;
        snapshot.account.identity_status = "unverified_cache";
        snapshot.data_status.auth = AuthState::Unknown;
        snapshot.data_status.freshness =
            if now.saturating_sub(snapshot.fetched_at_unix_seconds) >= 900 {
                Freshness::Stale
            } else {
                Freshness::Fresh
            };
        Ok(Some(CachedQuota {
            snapshot,
            account_key: envelope.account_key,
        }))
    }
}

#[derive(Deserialize)]
struct StoredSnapshot {
    fetched_at_unix_seconds: u64,
    source: StoredSource,
    buckets: Vec<StoredBucket>,
    ordinary_usage_allowed: Option<bool>,
}
#[derive(Deserialize)]
struct StoredSource {
    cli_version: String,
}
#[derive(Deserialize)]
struct StoredBucket {
    id: String,
    windows: Vec<StoredWindow>,
    spend_control_reached: Option<bool>,
}
#[derive(Deserialize)]
struct StoredWindow {
    source_role: String,
    used_percent: f64,
    window_duration_minutes: Option<i64>,
    resets_at_unix_seconds: Option<i64>,
}
