#![cfg(all(windows, feature = "test-support"))]
use std::{
    path::PathBuf,
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, WAIT_TIMEOUT},
    System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject},
};

static COUNTER: AtomicU64 = AtomicU64::new(0);
fn unique_file() -> PathBuf {
    std::env::temp_dir().join(format!(
        "quotarail-process-test-{}-{}.jsonl",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ))
}
fn is_running(id: u32) -> bool {
    let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, id) };
    if handle.is_null() {
        return false;
    }
    let running = unsafe { WaitForSingleObject(handle, 0) } == WAIT_TIMEOUT;
    unsafe {
        CloseHandle(handle);
    }
    running
}

#[test]
fn job_cleans_descendants_on_normal_exit_and_forced_parent_termination() {
    for force in [false, true] {
        let pid_file = unique_file();
        let metrics = unique_file();
        let mut command = Command::new(env!("CARGO_BIN_EXE_quota_probe"));
        command
            .args(["--codex", env!("CARGO_BIN_EXE_mock_codex"), "--json"])
            .env("QUOTARAIL_MOCK_SCENARIO", "descendant")
            .env("QUOTARAIL_MOCK_PID_FILE", &pid_file)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if force {
            command
                .args(["--observe-seconds", "60", "--metrics"])
                .arg(&metrics);
        }
        let mut probe = command.spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !pid_file.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(pid_file.exists(), "mock process tree was not created");
        let ids: Vec<u32> = std::fs::read_to_string(&pid_file)
            .unwrap()
            .split_whitespace()
            .map(|v| v.parse().unwrap())
            .collect();
        if force {
            probe.kill().unwrap();
        }
        let status = probe.wait().unwrap();
        if !force {
            assert!(status.success());
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        while ids.iter().any(|id| is_running(*id)) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            ids.iter().all(|id| !is_running(*id)),
            "owned descendants survived"
        );
        let _ = std::fs::remove_file(pid_file);
        let _ = std::fs::remove_file(metrics);
    }
}

#[test]
fn resource_observation_includes_parent_and_children_without_overwriting_files() {
    let metrics = unique_file();
    let output = Command::new(env!("CARGO_BIN_EXE_quota_probe"))
        .args([
            "--codex",
            env!("CARGO_BIN_EXE_mock_codex"),
            "--json",
            "--observe-seconds",
            "1",
            "--metrics",
        ])
        .arg(&metrics)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let body = std::fs::read_to_string(&metrics).unwrap();
    for line in body.lines() {
        let sample: serde_json::Value = serde_json::from_str(line).unwrap();
        assert!(sample["processes"].as_array().unwrap().len() >= 2);
        assert_eq!(sample["unavailable_processes"], 0);
        assert!(sample["total_private_bytes"].as_u64().unwrap() > 0);
    }
    let refused = Command::new(env!("CARGO_BIN_EXE_quota_probe"))
        .args([
            "--codex",
            env!("CARGO_BIN_EXE_mock_codex"),
            "--json",
            "--observe-seconds",
            "1",
            "--metrics",
        ])
        .arg(&metrics)
        .output()
        .unwrap();
    assert!(!refused.status.success());
    assert_eq!(std::fs::read_to_string(&metrics).unwrap(), body);
    std::fs::remove_file(metrics).unwrap();
}
