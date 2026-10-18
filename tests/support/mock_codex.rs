//! Synthetic stdio peer, built only with --features test-support. Never uses credentials.
use serde_json::{Value, json};
use std::io::{self, BufRead, Write};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--version") {
        println!("codex-cli 0.160.0");
        return;
    }
    if args.iter().any(|a| a == "--descendant") {
        std::thread::sleep(std::time::Duration::from_secs(60));
        return;
    }
    let scenario = std::env::var("QUOTARAIL_MOCK_SCENARIO").unwrap_or_default();
    let mut initialized = false;
    let mut acknowledged = false;
    let mut reads = 0;
    let mut refreshes = 0;
    for line in io::stdin().lock().lines() {
        let line = line.unwrap();
        let request: Value = serde_json::from_str(&line).unwrap();
        let method = request["method"].as_str().unwrap_or_default();
        if let Some(path) = std::env::var_os("QUOTARAIL_MOCK_REQUEST_LOG") {
            let mut log = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .unwrap();
            writeln!(log, "{} {method}", std::process::id()).unwrap();
        }
        if method == "initialized" {
            acknowledged = true;
            continue;
        }
        let id = request["id"].clone();
        let result = match method {
            "initialize" => {
                assert!(!initialized);
                initialized = true;
                if scenario == "descendant" {
                    use std::os::windows::process::CommandExt;
                    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
                        .arg("--descendant")
                        .creation_flags(0x08000000)
                        .spawn()
                        .unwrap();
                    std::fs::write(
                        std::env::var_os("QUOTARAIL_MOCK_PID_FILE").unwrap(),
                        format!("{} {}", std::process::id(), child.id()),
                    )
                    .unwrap();
                    // Reap normally if the synthetic worker outlives the parent; the lifecycle
                    // test still checks that the outer Windows job kills it when the peer exits.
                    std::thread::spawn(move || {
                        let _ = child.wait();
                    });
                }
                if scenario == "eof" {
                    return;
                }
                if scenario == "invalid_json" {
                    println!("synthetic invalid secret_message");
                    return;
                }
                if scenario == "oversized" {
                    println!("{}", "x".repeat(1024 * 1024 + 1));
                    return;
                }
                if scenario == "hang" {
                    std::thread::sleep(std::time::Duration::from_secs(40));
                    return;
                }
                json!({"userAgent":"synthetic"})
            }
            "account/read" => {
                assert!(acknowledged);
                if request["params"]["refreshToken"] == true {
                    refreshes += 1;
                    assert_eq!(refreshes, 1, "Recovery must be bounded");
                    assert!(scenario == "recover_once" || scenario == "persistent_401");
                }
                if scenario == "signed_out" {
                    json!({"account":null})
                } else if scenario == "api_key" {
                    json!({"account":{"type":"apiKey"}})
                } else {
                    json!({"account":{"type":"chatgpt","email":"SYNTHETIC_PRIVATE_EMAIL","unknownFutureField":true}})
                }
            }
            "account/rateLimits/read" => {
                reads += 1;
                if let Ok(delay) = std::env::var("QUOTARAIL_MOCK_DELAY_MS") {
                    let millis = delay.parse::<u64>().unwrap().min(5000);
                    std::thread::sleep(std::time::Duration::from_millis(millis));
                }
                if scenario == "error"
                    || (scenario == "recover_once" && reads == 1)
                    || scenario == "persistent_401"
                    || scenario == "forbidden"
                {
                    let message = match scenario.as_str() {
                        "recover_once" | "persistent_401" => "401 SYNTHETIC_SECRET_ACCESS_TOKEN",
                        "forbidden" => "403 SYNTHETIC_SECRET_ACCESS_TOKEN",
                        _ => "SYNTHETIC_SECRET_ACCESS_TOKEN",
                    };
                    println!(
                        "{}",
                        json!({"id":id,"error":{"code":-32603,"message":message}})
                    );
                    io::stdout().flush().unwrap();
                    continue;
                }
                if scenario == "widget" {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_secs();
                    json!({"accountId":"SYNTHETIC_PRIVATE_ACCOUNT","rateLimits":{"limitId":"codex","primary":{"usedPercent":42.0,"windowDurationMins":300,"resetsAt":now+8040},"secondary":{"usedPercent":68.0,"windowDurationMins":10080,"resetsAt":now+280800}}})
                } else {
                    json!({"accountId":"SYNTHETIC_PRIVATE_ACCOUNT","rateLimits":{"limitId":"codex","primary":{"usedPercent":31.25,"windowDurationMins":10080,"resetsAt":null},"secondary":null},"futureField":"ignored"})
                }
            }
            _ => {
                panic!("Unexpected method; the probe must not create threads, turns or login flows")
            }
        };
        println!(
            "{}",
            json!({"method":"account/updated","params":{"ignored":"SYNTHETIC_PRIVATE_EMAIL"}})
        );
        if scenario == "partial" {
            let frame = json!({"id":id,"result":result}).to_string();
            for chunk in frame.as_bytes().chunks(3) {
                io::stdout().write_all(chunk).unwrap();
                io::stdout().flush().unwrap();
            }
            println!();
        } else {
            println!("{}", json!({"id":id,"result":result}));
        }
        eprintln!("SYNTHETIC_SECRET_STDERR");
        io::stdout().flush().unwrap();
    }
}
