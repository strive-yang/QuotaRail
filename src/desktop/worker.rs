use super::{
    DesktopOptions,
    state::{DesktopState, auth_requires_action, retry_delay},
};
use crate::{
    discovery::find_codex,
    error::{ErrorKind, ProbeError, Result},
    process::detect_version,
    protocol::AppServer,
    provider::read_snapshot,
    storage::{CacheStore, unix_now},
};
use std::{
    sync::{Arc, Mutex},
    thread::JoinHandle,
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, watch};

#[derive(Clone, Copy)]
pub enum Command {
    Refresh,
    Reconnect,
    Wake,
    Shutdown,
}

pub type SharedState = Arc<Mutex<DesktopState>>;

pub struct Worker {
    pub sender: mpsc::Sender<Command>,
    thread: Option<JoinHandle<()>>,
    refresh: watch::Sender<u32>,
}

impl Worker {
    pub fn start(
        options: DesktopOptions,
        shared: SharedState,
        notify: impl Fn() + Send + 'static,
    ) -> Result<Self> {
        let (sender, receiver) = mpsc::channel(8);
        let (refresh, refresh_receiver) = watch::channel(options.refresh_minutes);
        let thread = std::thread::Builder::new()
            .name("quotarail-provider".into())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build();
                match runtime {
                    Ok(runtime) => {
                        runtime.block_on(run(options, shared, receiver, refresh_receiver, notify))
                    }
                    Err(_) => {
                        let mut state = shared.lock().unwrap_or_else(|e| e.into_inner());
                        state.error = Some(ProbeError::new(
                            ErrorKind::Io,
                            "Background runtime is unavailable.",
                        ));
                        drop(state);
                        notify();
                    }
                }
            })
            .map_err(|_| ProbeError::new(ErrorKind::Io, "Cannot start the quota worker."))?;
        Ok(Self {
            sender,
            thread: Some(thread),
            refresh,
        })
    }

    pub fn set_refresh_minutes(&self, minutes: u32) {
        self.refresh.send_replace(minutes);
    }
    pub fn is_running(&self) -> bool {
        self.thread.is_some()
    }

    pub fn shutdown(&mut self) {
        let _ = self.sender.blocking_send(Command::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        if self.thread.is_some() {
            self.shutdown();
        }
    }
}

struct Session {
    version: String,
    server: AppServer,
}

async fn fetch(
    options: &DesktopOptions,
    session: &mut Option<Session>,
) -> Result<crate::domain::QuotaSnapshot> {
    if session.is_none() {
        let exe = find_codex(options.codex.as_deref(), std::env::var_os("PATH"))?;
        let version = detect_version(&exe, Some(&options.home)).await?;
        let server = AppServer::start(&exe, Some(&options.home)).await?;
        *session = Some(Session { version, server });
    }
    let session = session
        .as_mut()
        .ok_or_else(|| ProbeError::new(ErrorKind::Io, "Session is unavailable."))?;
    read_snapshot(&mut session.server, &session.version).await
}

async fn run(
    options: DesktopOptions,
    shared: SharedState,
    mut receiver: mpsc::Receiver<Command>,
    mut refresh: watch::Receiver<u32>,
    notify: impl Fn(),
) {
    let resolved = find_codex(options.codex.as_deref(), std::env::var_os("PATH"));
    let cache = resolved
        .as_ref()
        .ok()
        .and_then(|exe| CacheStore::new(&options.data_dir, exe, &options.home).ok());
    {
        let mut state = shared.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(cache) = &cache {
            match cache.load(unix_now()) {
                Ok(Some(cached)) => state.history = Some(cached.snapshot),
                Ok(None) => {}
                Err(_) => state.cache_warning = true,
            }
        } else {
            state.cache_warning = true;
        }
    }
    notify();
    let mut session = None;
    let mut deadline = Some(Instant::now());
    let mut failures = 0u32;
    let mut manual_deadline = Instant::now();
    let mut refresh_minutes = options.refresh_minutes;
    loop {
        if deadline.is_some_and(|due| Instant::now() >= due) {
            let generation = {
                let mut state = shared.lock().unwrap_or_else(|e| e.into_inner());
                state.loading = true;
                state.manual_available_at = unix_now().saturating_add(30);
                state.generation
            };
            notify();
            let result = {
                let request =
                    tokio::time::timeout(Duration::from_secs(30), fetch(&options, &mut session));
                tokio::pin!(request);
                loop {
                    tokio::select! {
                        result = &mut request => break result.unwrap_or_else(|_| Err(ProbeError::new(ErrorKind::Timeout, "Background quota read timed out."))),
                        command = receiver.recv() => match command {
                            Some(Command::Shutdown) | None => return,
                            // Manual/automatic intents arriving during the same read are coalesced.
                            Some(Command::Refresh | Command::Reconnect | Command::Wake) => {},
                        },
                        _ = refresh.changed() => {
                            refresh_minutes = *refresh.borrow_and_update();
                            shared.lock().unwrap_or_else(|e| e.into_inner()).refresh_minutes = refresh_minutes;
                            notify();
                        }
                    }
                }
            };
            let is_error = result.is_err();
            let mut wait_for_login = false;
            if let Ok(snapshot) = &result {
                if let Some(cache) = &cache {
                    let saved = cache.save(snapshot);
                    shared
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .cache_warning = !matches!(saved, Ok(true));
                }
                failures = 0;
            } else if let Err(error) = &result {
                failures = failures.saturating_add(1);
                wait_for_login = auth_requires_action(error.kind);
                // Drop a potentially poisoned connection; the next scheduled attempt owns a fresh job.
                session = None;
            }
            {
                let mut state = shared.lock().unwrap_or_else(|e| e.into_inner());
                state.apply(generation, result);
                state.manual_available_at = unix_now().saturating_add(30);
            }
            notify();
            manual_deadline = Instant::now() + Duration::from_secs(30);
            let jitter = u64::from(std::process::id()) ^ unix_now();
            deadline = if wait_for_login {
                None
            } else {
                Some(
                    Instant::now()
                        + Duration::from_secs(if is_error {
                            retry_delay(failures, jitter)
                        } else {
                            u64::from(refresh_minutes) * 60 + jitter % 16
                        }),
                )
            };
        }
        let wait = deadline
            .map(|at| at.saturating_duration_since(Instant::now()))
            .unwrap_or(Duration::from_secs(86400));
        tokio::select! {
            _ = tokio::time::sleep(wait) => {},
            _ = refresh.changed() => {
                refresh_minutes = *refresh.borrow_and_update();
                shared.lock().unwrap_or_else(|e| e.into_inner()).refresh_minutes = refresh_minutes;
                if deadline.is_some() {
                    deadline = Some(Instant::now() + Duration::from_secs(u64::from(refresh_minutes) * 60));
                }
                notify();
            },
            command = receiver.recv() => match command {
                Some(Command::Shutdown) | None => {
                    if let Some(session) = &mut session { let _ = session.server.process.stop().await; }
                    return;
                }
                Some(Command::Refresh | Command::Wake) if Instant::now() >= manual_deadline => deadline = Some(Instant::now()),
                Some(Command::Reconnect) if Instant::now() >= manual_deadline => {
                    session = None;
                    shared.lock().unwrap_or_else(|e| e.into_inner()).reconnect();
                    deadline = Some(Instant::now());
                    notify();
                }
                _ => {},
            }
        }
    }
}
