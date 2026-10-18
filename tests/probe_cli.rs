#![cfg(feature = "test-support")]
use serde_json::Value;
use std::process::Command;

fn run(scenario: &str) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_quota_probe"))
        .args(["--codex", env!("CARGO_BIN_EXE_mock_codex"), "--json"])
        .env("QUOTARAIL_MOCK_SCENARIO", scenario)
        .output()
        .unwrap()
}

#[test]
fn real_process_handshake_accepts_fragmented_frames_and_ignores_notifications() {
    for scenario in ["", "partial", "recover_once"] {
        let output = run(scenario);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["snapshot"]["buckets"][0]["windows"][0]["label"], "7D");
        assert!(!String::from_utf8_lossy(&output.stdout).contains("SYNTHETIC_PRIVATE"));
        assert!(output.stderr.is_empty());
    }
}

#[test]
fn failures_are_explicit_and_never_export_upstream_secrets_or_zero_usage() {
    for (scenario, kind, code) in [
        ("signed_out", "signed_out", 3),
        ("api_key", "unsupported_auth", 3),
        ("error", "request_failed", 1),
        ("persistent_401", "authentication_rejected", 3),
        ("forbidden", "access_denied", 1),
        ("eof", "child_exited", 1),
        ("invalid_json", "protocol", 1),
        ("oversized", "message_too_large", 1),
        ("hang", "timeout", 4),
    ] {
        let output = run(scenario);
        assert_eq!(output.status.code(), Some(code));
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["error"]["kind"], kind);
        assert!(value.get("snapshot").is_none());
        assert!(!String::from_utf8_lossy(&output.stdout).contains("SYNTHETIC_SECRET"));
        assert!(!String::from_utf8_lossy(&output.stdout).contains("secret_message"));
        assert!(output.stderr.is_empty());
    }
}

#[test]
fn invalid_cli_options_do_not_launch_a_server() {
    let output = Command::new(env!("CARGO_BIN_EXE_quota_probe"))
        .args(["--json", "--observe-seconds", "0"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
}
