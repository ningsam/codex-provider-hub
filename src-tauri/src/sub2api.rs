//! Sub2API OpenAI account-pool usage via `./sub2api metrics`.

use crate::gateway::sub2api_dir;
use crate::http_util::now_iso;
use serde::{Deserialize, Serialize};
use std::process::Command;
use std::time::Duration;

/// Remaining quota window for Sub2API (5h or 7d).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaWindow {
    pub remaining_percent: f64,
    pub reset_after_seconds: u64,
}

/// Aggregated Sub2API pool usage snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Sub2ApiUsage {
    pub five_hour: QuotaWindow,
    pub seven_day: QuotaWindow,
    pub pool_total: u32,
    pub pool_available: u32,
    pub fetched_at: String,
}

fn run_metrics_json() -> Result<serde_json::Value, String> {
    let dir = sub2api_dir();
    if !dir.is_dir() {
        return Err(format!(
            "sub2api directory missing: {} (set SUB2API_DIR)",
            dir.display()
        ));
    }
    let output = Command::new("./sub2api")
        .arg("metrics")
        .current_dir(&dir)
        .output()
        .map_err(|e| format!("spawn ./sub2api metrics: {e}"))?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "./sub2api metrics failed: {}",
            err.trim().chars().take(200).collect::<String>()
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Prefer the last JSON object line (script may print logs before JSON).
    let json_line = stdout
        .lines()
        .rev()
        .find(|l| l.trim_start().starts_with('{'))
        .unwrap_or(stdout.trim());
    serde_json::from_str(json_line).map_err(|e| format!("parse metrics JSON: {e}"))
}

pub fn fetch_sub2api_usage() -> Result<Sub2ApiUsage, String> {
    let v = run_metrics_json()?;
    if v.get("ok").and_then(|o| o.as_bool()) == Some(false) {
        return Err("sub2api metrics reported ok=false".into());
    }

    let pool = v.get("pool").cloned().unwrap_or(serde_json::json!({}));
    let quota = v.get("quota").cloned().unwrap_or(serde_json::json!({}));
    let five = quota.get("five_hour").cloned().unwrap_or_default();
    let seven = quota.get("seven_day").cloned().unwrap_or_default();

    Ok(Sub2ApiUsage {
        five_hour: QuotaWindow {
            remaining_percent: five
                .get("average_remaining")
                .and_then(|x| x.as_f64())
                .unwrap_or(0.0),
            reset_after_seconds: five
                .get("reset_after_seconds")
                .and_then(|x| x.as_u64())
                .unwrap_or(0),
        },
        seven_day: QuotaWindow {
            remaining_percent: seven
                .get("average_remaining")
                .and_then(|x| x.as_f64())
                .unwrap_or(0.0),
            reset_after_seconds: seven
                .get("reset_after_seconds")
                .and_then(|x| x.as_u64())
                .unwrap_or(0),
        },
        pool_total: pool.get("total").and_then(|x| x.as_u64()).unwrap_or(0) as u32,
        pool_available: pool.get("available").and_then(|x| x.as_u64()).unwrap_or(0) as u32,
        fetched_at: now_iso(),
    })
}

/// Fetch Sub2API 5-hour / 7-day remaining quota and pool availability.
#[tauri::command]
pub fn get_sub2api_usage() -> Result<Sub2ApiUsage, String> {
    crate::http_util::cached_json("sub2api_usage", Duration::from_secs(30), fetch_sub2api_usage)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_metrics() {
        let u = fetch_sub2api_usage().expect("metrics");
        println!(
            "5h={}% reset={}s 7d={}% pool={}/{}",
            u.five_hour.remaining_percent,
            u.five_hour.reset_after_seconds,
            u.seven_day.remaining_percent,
            u.pool_available,
            u.pool_total
        );
        assert!(u.pool_total > 0);
    }
}
