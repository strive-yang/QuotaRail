use crate::{
    error::{ErrorKind, ProbeError, Result},
    process::ManagedChild,
};
use serde_json::{Value, json};
use std::{path::Path, time::Duration};
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::ChildStdout,
};

pub const MAX_MESSAGE_BYTES: usize = 1024 * 1024;

pub async fn read_frame<R: AsyncBufRead + Unpin>(reader: &mut R) -> Result<Value> {
    let mut bytes = Vec::new();
    loop {
        let available = reader
            .fill_buf()
            .await
            .map_err(|_| ProbeError::new(ErrorKind::Io, "Cannot read protocol stream."))?;
        if available.is_empty() {
            return Err(ProbeError::new(
                ErrorKind::ChildExited,
                "Protocol stream closed before a complete response.",
            ));
        }
        let consumed = available
            .iter()
            .position(|b| *b == b'\n')
            .map_or(available.len(), |i| i + 1);
        if bytes.len() + consumed > MAX_MESSAGE_BYTES {
            return Err(ProbeError::new(
                ErrorKind::MessageTooLarge,
                "Protocol message exceeded the size limit.",
            ));
        }
        bytes.extend_from_slice(&available[..consumed]);
        reader.consume(consumed);
        if bytes.last() == Some(&b'\n') {
            return serde_json::from_slice(&bytes).map_err(|_| {
                ProbeError::new(
                    ErrorKind::Protocol,
                    "Invalid protocol JSON; raw content suppressed.",
                )
            });
        }
    }
}

pub struct AppServer {
    pub process: ManagedChild,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
    poisoned: bool,
}

impl AppServer {
    pub async fn start(exe: &Path, home: Option<&Path>) -> Result<Self> {
        let mut process = ManagedChild::spawn(exe, &["app-server", "--listen", "stdio://"], home)?;
        let stdout = process
            .child
            .stdout
            .take()
            .ok_or_else(|| ProbeError::new(ErrorKind::Io, "Protocol stdout is unavailable."))?;
        let mut server = Self {
            process,
            stdout: BufReader::new(stdout),
            next_id: 0,
            poisoned: false,
        };
        server.request("initialize", json!({"clientInfo":{"name":"quotarail", "title":"QuotaRail", "version":env!("CARGO_PKG_VERSION")}, "capabilities":{"experimentalApi":false}}), Duration::from_secs(10)).await?;
        server
            .send(&json!({"method":"initialized","params":{}}))
            .await?;
        Ok(server)
    }

    async fn send(&mut self, value: &Value) -> Result<()> {
        let mut data = serde_json::to_vec(value)
            .map_err(|_| ProbeError::new(ErrorKind::Protocol, "Cannot encode protocol request."))?;
        data.push(b'\n');
        let stdin =
            self.process.child.stdin.as_mut().ok_or_else(|| {
                ProbeError::new(ErrorKind::ChildExited, "Protocol stdin is closed.")
            })?;
        stdin
            .write_all(&data)
            .await
            .map_err(|_| ProbeError::new(ErrorKind::Io, "Cannot send protocol request."))?;
        stdin
            .flush()
            .await
            .map_err(|_| ProbeError::new(ErrorKind::Io, "Cannot flush protocol request."))
    }

    pub async fn request(
        &mut self,
        method: &'static str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value> {
        if self.poisoned {
            return Err(ProbeError::new(
                ErrorKind::Protocol,
                "Timed-out or invalid connection cannot be reused.",
            ));
        }
        self.next_id += 1;
        let id = self.next_id;
        let exchange = async {
            self.send(&json!({"id":id,"method":method,"params":params}))
                .await?;
            for _ in 0..256 {
                let message = read_frame(&mut self.stdout).await?;
                if !message.is_object() {
                    return Err(ProbeError::new(
                        ErrorKind::Protocol,
                        "Protocol frame must be an object.",
                    ));
                }
                if message.get("method").and_then(Value::as_str).is_some() {
                    if let Some(server_id) = message.get("id") {
                        self.send(&json!({"id":server_id,"error":{"code":-32601,"message":"QuotaRail does not implement server requests"}})).await?;
                    }
                    continue;
                }
                if message.get("id").and_then(Value::as_u64) != Some(id) {
                    continue;
                }
                if let Some(error) = message.get("error") {
                    return Err(classify_rpc_error(error));
                }
                return message.get("result").cloned().ok_or_else(|| {
                    ProbeError::new(ErrorKind::Protocol, "Protocol response omitted its result.")
                });
            }
            Err(ProbeError::new(
                ErrorKind::Protocol,
                "Too many unrelated protocol messages.",
            ))
        };
        let result = tokio::time::timeout(timeout, exchange)
            .await
            .unwrap_or_else(|_| {
                Err(ProbeError::new(
                    ErrorKind::Timeout,
                    "Protocol request timed out.",
                ))
            });
        if let Err(error) = &result {
            self.poisoned = matches!(
                error.kind,
                ErrorKind::Timeout
                    | ErrorKind::Protocol
                    | ErrorKind::MessageTooLarge
                    | ErrorKind::ChildExited
                    | ErrorKind::Io
            );
        }
        result
    }
}

fn classify_rpc_error(error: &Value) -> ProbeError {
    let code = error.get("code").and_then(Value::as_i64);
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default();
    // Examine only to classify; never include upstream text in diagnostics.
    let lower = message.to_ascii_lowercase();
    let (kind, safe_message) = if code == Some(-32601) {
        (
            ErrorKind::UnsupportedVersion,
            "This CLI does not implement the required method.",
        )
    } else if lower.contains("401") {
        (
            ErrorKind::AuthenticationRejected,
            "Codex authentication was rejected; update login in Codex.",
        )
    } else if lower.contains("403") {
        (
            ErrorKind::AccessDenied,
            "Access denied; authentication, policy or network filtering may be responsible.",
        )
    } else if lower.contains("authentication required") {
        (
            ErrorKind::SignedOut,
            "Codex account authentication is required.",
        )
    } else {
        (
            ErrorKind::RequestFailed,
            "Codex request failed; upstream details suppressed.",
        )
    };
    ProbeError::new(kind, safe_message)
}
