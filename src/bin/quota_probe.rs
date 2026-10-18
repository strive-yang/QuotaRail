use quotarail::{
    discovery::find_codex,
    domain::{Account, AuthState, DataStatus, FetchState, Freshness},
    error::{ErrorKind, ProbeError, Result},
    process::detect_version,
    protocol::AppServer,
    provider::read_snapshot,
};
use serde_json::json;
use std::{
    ffi::OsString,
    fs::{File, OpenOptions},
    io::{BufWriter, Write},
    path::PathBuf,
    time::{Duration, Instant},
};

#[derive(Default)]
struct Options {
    codex: Option<PathBuf>,
    home: Option<PathBuf>,
    json: bool,
    observe_seconds: u64,
    metrics: Option<PathBuf>,
}

fn invalid_args() -> ProbeError {
    ProbeError::new(
        ErrorKind::InvalidArguments,
        "Invalid arguments; see quota_probe --help.",
    )
}

fn parse_options(args: impl Iterator<Item = OsString>) -> Result<Option<Options>> {
    let mut args = args.peekable();
    let mut options = Options::default();
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--help" | "-h") => {
                println!(
                    "QuotaRail P0 quota probe\n\nUsage: quota_probe [--codex PATH] [--codex-home PATH] [--json]\n                   [--observe-seconds N --metrics NEW.jsonl]\n\nDefaults: one read, native Codex from PATH, no stored credentials.\nObservation: sample owned process resources every second; query every 300s.\nMetrics contains resource counters only; output file must not already exist.\nNo threads/turns, direct HTTP or login/logout commands are issued."
                );
                return Ok(None);
            }
            Some("--version" | "-V") => {
                println!("quota_probe {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            Some("--json") => options.json = true,
            Some("--codex") => {
                options.codex = Some(PathBuf::from(args.next().ok_or_else(invalid_args)?))
            }
            Some("--codex-home") => {
                options.home = Some(PathBuf::from(args.next().ok_or_else(invalid_args)?))
            }
            Some("--metrics") => {
                options.metrics = Some(PathBuf::from(args.next().ok_or_else(invalid_args)?))
            }
            Some("--observe-seconds") => {
                options.observe_seconds = args
                    .next()
                    .and_then(|v| v.to_str().and_then(|s| s.parse().ok()))
                    .filter(|n| (1..=86400).contains(n))
                    .ok_or_else(invalid_args)?;
            }
            _ => return Err(invalid_args()),
        }
    }
    if (options.observe_seconds > 0) != options.metrics.is_some() {
        return Err(invalid_args());
    }
    if let Some(home) = &options.home {
        options.home = Some(home.canonicalize().map_err(|_| {
            ProbeError::new(
                ErrorKind::InvalidArguments,
                "CODEX_HOME must be an existing directory.",
            )
        })?);
        if !options.home.as_ref().is_some_and(|p| p.is_dir()) {
            return Err(invalid_args());
        }
    }
    Ok(Some(options))
}

fn metric_writer(path: &PathBuf) -> Result<BufWriter<File>> {
    if !path
        .extension()
        .is_some_and(|v| v.eq_ignore_ascii_case("jsonl"))
    {
        return Err(invalid_args());
    }
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|_| {
            ProbeError::new(
                ErrorKind::Io,
                "Metrics needs a new .jsonl file in an existing directory.",
            )
        })?;
    Ok(BufWriter::new(file))
}

fn output_snapshot(
    snapshot: &quotarail::domain::QuotaSnapshot,
    as_json: bool,
    elapsed_ms: u128,
) -> Result<()> {
    let output = if as_json {
        json!({"status":"ok","elapsed_ms":elapsed_ms,"snapshot":snapshot}).to_string()
    } else {
        let mut lines = vec![format!(
            "Codex {} | account: {} | fetched Unix seconds: {}",
            snapshot.source.cli_version,
            snapshot.account.identity_status,
            snapshot.fetched_at_unix_seconds
        )];
        for bucket in &snapshot.buckets {
            lines.push(format!(
                "Bucket: {} | 5H: {:?} | 7D: {:?}",
                bucket.id, bucket.five_hour, bucket.seven_day
            ));
            for window in &bucket.windows {
                lines.push(format!(
                    "  {}: used {:.2}%, remaining {:.2}%, reset Unix seconds: {:?}{}",
                    window.label,
                    window.used_percent,
                    window.remaining_percent,
                    window.resets_at_unix_seconds,
                    if window.percent_out_of_range {
                        " [invalid percent range]"
                    } else {
                        ""
                    }
                ));
            }
        }
        lines.join("\n")
    };
    writeln!(std::io::stdout().lock(), "{output}")
        .map_err(|_| ProbeError::new(ErrorKind::Io, "Cannot write probe output."))
}

async fn run(options: Options) -> Result<()> {
    let exe = find_codex(options.codex.as_deref(), std::env::var_os("PATH"))?;
    let mut metrics = options.metrics.as_ref().map(metric_writer).transpose()?;
    let started = Instant::now();
    let startup = async {
        let version = detect_version(&exe, options.home.as_deref()).await?;
        let mut server = AppServer::start(&exe, options.home.as_deref()).await?;
        let snapshot = read_snapshot(&mut server, &version).await?;
        Ok::<_, ProbeError>((version, server, snapshot))
    };
    let (version, mut server, snapshot) = tokio::time::timeout(Duration::from_secs(30), startup)
        .await
        .map_err(|_| {
            ProbeError::new(
                ErrorKind::Timeout,
                "Initial probe exceeded its total 30-second deadline.",
            )
        })??;
    output_snapshot(&snapshot, options.json, started.elapsed().as_millis())?;
    let mut previous_account: Account = snapshot.account;
    if options.observe_seconds > 0 {
        let observation_started = Instant::now();
        let mut next_read = Duration::from_secs(300);
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            let elapsed = observation_started.elapsed();
            let mut sample = server.process.sample()?;
            sample.elapsed_seconds = elapsed.as_secs_f64();
            if let Some(writer) = &mut metrics {
                serde_json::to_writer(&mut *writer, &sample).map_err(|_| {
                    ProbeError::new(ErrorKind::Io, "Cannot encode resource sample.")
                })?;
                writeln!(writer)
                    .and_then(|()| writer.flush())
                    .map_err(|_| ProbeError::new(ErrorKind::Io, "Cannot write resource sample."))?;
            }
            if elapsed >= Duration::from_secs(options.observe_seconds) {
                break;
            }
            if elapsed >= next_read {
                let request_started = Instant::now();
                let snapshot = tokio::time::timeout(
                    Duration::from_secs(30),
                    read_snapshot(&mut server, &version),
                )
                .await
                .map_err(|_| {
                    ProbeError::new(
                        ErrorKind::Timeout,
                        "Observation quota read exceeded its deadline.",
                    )
                })??;
                if previous_account.same_backend_identity(&snapshot.account) == Some(false) {
                    return Err(ProbeError::new(
                        ErrorKind::RequestFailed,
                        "Backend account changed; restart the probe for the new account.",
                    ));
                }
                output_snapshot(
                    &snapshot,
                    options.json,
                    request_started.elapsed().as_millis(),
                )?;
                previous_account = snapshot.account;
                next_read = observation_started.elapsed() + Duration::from_secs(300);
            }
        }
    }
    server.process.stop().await
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let as_json = args.iter().any(|a| a == "--json");
    let result = match parse_options(args.into_iter()) {
        Ok(Some(options)) => {
            tokio::select! {
                result = run(options) => result,
                signal = tokio::signal::ctrl_c() => {
                    let _ = signal;
                    Err(ProbeError::new(ErrorKind::Cancelled, "Probe cancelled; owned processes are being released."))
                }
            }
        }
        Ok(None) => Ok(()),
        Err(error) => Err(error),
    };
    if let Err(error) = result {
        let data_status = DataStatus {
            fetch: FetchState::Failed,
            auth: match error.kind {
                ErrorKind::SignedOut => AuthState::SignedOut,
                ErrorKind::AuthenticationRejected => AuthState::Rejected,
                ErrorKind::UnsupportedAuth => AuthState::Unsupported,
                _ => AuthState::Unknown,
            },
            freshness: Freshness::NoData,
        };
        if as_json {
            let _ = writeln!(
                std::io::stdout().lock(),
                "{}",
                json!({"status":"error","data_status":data_status,"error":error})
            );
        } else {
            let _ = writeln!(std::io::stderr().lock(), "{error}");
        }
        // All run futures (and their process jobs) have been dropped before process::exit.
        std::process::exit(error.exit_code());
    }
}
