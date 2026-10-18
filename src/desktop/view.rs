use super::state::DesktopState;
use crate::{
    domain::{LimitBucket, LimitWindow, QuotaSnapshot, WindowAvailability},
    error::ErrorKind,
};

pub fn status(state: &DesktopState, now: u64) -> &'static str {
    if state.loading {
        return "刷新中";
    }
    if let Some(error) = &state.error {
        return match error.kind {
            ErrorKind::SignedOut => "未登录",
            ErrorKind::AuthenticationRejected => "登录需更新",
            ErrorKind::UnsupportedAuth => "认证方式不支持",
            _ if state.snapshot.is_some() => "刷新失败 · 保留上次数据",
            _ => "读取失败",
        };
    }
    if let Some(snapshot) = &state.snapshot {
        if snapshot.ordinary_usage_allowed == Some(false)
            || snapshot
                .buckets
                .iter()
                .any(|b| b.spend_control_reached == Some(true))
        {
            return "服务端用量受限";
        }
        if snapshot
            .buckets
            .iter()
            .any(|b| b.windows.iter().any(|w| w.percent_out_of_range))
        {
            return "额度比例异常";
        }
        if now.saturating_sub(snapshot.fetched_at_unix_seconds)
            >= u64::from(state.refresh_minutes.max(5)) * 180
        {
            "缓存过期"
        } else {
            "正常"
        }
    } else {
        "尚无数据"
    }
}

pub fn primary_bucket(snapshot: &QuotaSnapshot) -> Option<&LimitBucket> {
    snapshot
        .buckets
        .iter()
        .find(|b| b.id == "codex")
        .or_else(|| snapshot.buckets.first())
}

/// The UI displays remaining quota. Invalid backend values stay unknown rather
/// than becoming an apparently valid 0% or 100% allowance.
pub fn remaining_percent(window: &LimitWindow) -> Option<f64> {
    (!window.percent_out_of_range).then_some(window.remaining_percent)
}

pub fn window_values(window: &LimitWindow) -> String {
    match remaining_percent(window) {
        Some(remaining) => format!("剩余 {remaining:.1}% · 已用 {:.1}%", window.used_percent),
        None => format!(
            "剩余未知 · 服务端已用 {:.1}%（比例异常）",
            window.used_percent
        ),
    }
}

pub fn window_line(bucket: &LimitBucket, minutes: i64) -> String {
    let label = if minutes == 300 { "5H" } else { "7D" };
    if let Some(window) = bucket
        .windows
        .iter()
        .find(|w| w.window_duration_minutes == Some(minutes))
    {
        format!("{label} {}", window_values(window))
    } else {
        let availability = if minutes == 300 {
            bucket.five_hour
        } else {
            bucket.seven_day
        };
        format!(
            "{label} — {}",
            if availability == WindowAvailability::UnidentifiedDuration {
                "窗口时长无法识别"
            } else {
                "本次未提供"
            }
        )
    }
}

pub fn tooltip(state: &DesktopState, now: u64) -> String {
    let mut text = format!("QuotaRail · {}", status(state, now));
    if let Some(snapshot) = &state.snapshot {
        if let Some(bucket) = primary_bucket(snapshot) {
            if bucket.id != "codex" {
                text.push_str(&format!("\n额度桶 {}", bucket.id));
            }
            for minutes in [300, 10080] {
                text.push_str(&format!("\n{}", window_line(bucket, minutes)));
            }
        }
        text.push_str(&format!(
            "\n上次成功 {} 分钟前",
            now.saturating_sub(snapshot.fetched_at_unix_seconds) / 60
        ));
    } else if state.history.is_some() {
        text.push_str("\n有历史缓存，账户尚未核验");
    }
    text
}

pub fn details(state: &DesktopState, now: u64, local_time: impl Fn(i64) -> String) -> String {
    let mut text = format!("账户状态：{}\n", status(state, now));
    if let Some(snapshot) = &state.snapshot {
        text.push_str(&snapshot_details(snapshot, now, &local_time));
    } else {
        text.push_str("当前账户尚无已确认的额度数据。\n");
    }
    if state.loading {
        text.push_str("\n正在后台刷新，详情不会额外发起请求。\n");
    }
    if let Some(error) = &state.error {
        let message = match error.kind {
            ErrorKind::SignedOut => "请先在 Windows 原生 Codex CLI 中登录，然后刷新。",
            ErrorKind::AuthenticationRejected => "请在 Codex 更新登录，再选择重新连接。",
            ErrorKind::UnsupportedAuth => "当前仅支持 Codex 管理的 ChatGPT 订阅登录。",
            ErrorKind::Timeout => "读取超时，可稍后手动刷新。",
            ErrorKind::CliNotFound | ErrorKind::UnsupportedLauncher => {
                "未找到受支持的 Codex 原生程序。"
            }
            ErrorKind::AccessDenied => "访问被拒绝，可能与网络、账户或策略有关。",
            _ => "本次读取未成功；没有将失败解释为 0%。",
        };
        text.push_str(&format!("\n{message}\n"));
    }
    if state.cache_warning {
        text.push_str("\n本地缓存不可用；在线结果仍可查看。\n");
    }
    text.push_str("\n自动刷新约每 5 分钟一次。可右键手动刷新或退出。");
    text
}

pub fn historical_details(
    state: &DesktopState,
    now: u64,
    local_time: impl Fn(i64) -> String,
) -> String {
    match &state.history {
        Some(snapshot) => format!(
            "上次成功数据（历史记录）\n账户未重新核验，不代表当前账户额度。\n\n{}",
            snapshot_details(snapshot, now, &local_time)
        ),
        None => "没有可用的历史缓存。".to_owned(),
    }
}

fn snapshot_details(
    snapshot: &QuotaSnapshot,
    now: u64,
    local_time: &impl Fn(i64) -> String,
) -> String {
    let mut text = String::new();
    for bucket in &snapshot.buckets {
        text.push_str(&format!("\n额度桶：{}\n", bucket.id));
        for window in &bucket.windows {
            text.push_str(&format!("{} {}\n", window.label, window_values(window)));
            if let Some(reset) = window.resets_at_unix_seconds {
                text.push_str(&format!("重置：{}（本地时间）\n", local_time(reset)));
                let seconds = u64::try_from(reset).unwrap_or_default().saturating_sub(now);
                if seconds == 0 {
                    text.push_str("倒计时：已到重置时刻，等待服务端更新\n");
                } else {
                    text.push_str(&format!(
                        "倒计时：{}天 {}小时 {}分\n",
                        seconds / 86400,
                        seconds % 86400 / 3600,
                        seconds % 3600 / 60
                    ));
                }
            } else {
                text.push_str("重置时间：未提供\n");
            }
        }
        if bucket.windows.is_empty() {
            text.push_str("未提供可显示的窗口。\n");
        }
        if bucket.spend_control_reached == Some(true) {
            text.push_str("服务端报告已达到支出限制。\n");
        }
    }
    if snapshot.ordinary_usage_allowed == Some(false) {
        text.push_str("服务端报告当前不允许普通包含用量；不能仅由百分比判断恢复。\n");
    }
    text.push_str(&format!(
        "\n上次成功：{}\n数据年龄：{}分钟\n来源：Windows Codex CLI {} / App Server\n",
        local_time(snapshot.fetched_at_unix_seconds as i64),
        now.saturating_sub(snapshot.fetched_at_unix_seconds) / 60,
        snapshot.source.cli_version
    ));
    text
}
