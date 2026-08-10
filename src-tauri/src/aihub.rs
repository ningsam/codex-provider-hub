//! AIHub (中转站) account balance via ANYROUTER_API_KEY.

use crate::http_util::{friendly_http_err, now_iso, HTTP};
use serde::{Deserialize, Serialize};
use std::fs;
use std::time::Duration;

/// AIHub wallet / usage balance snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AihubBalance {
    pub balance: f64,
    pub used: f64,
    pub currency: String,
    pub fetched_at: String,
}

fn read_anyrouter_key() -> Result<String, String> {
    if let Ok(k) = std::env::var("ANYROUTER_API_KEY") {
        let k = k.trim().to_string();
        if !k.is_empty() {
            return Ok(k);
        }
    }
    let zshrc = dirs::home_dir()
        .ok_or_else(|| "HOME not set".to_string())?
        .join(".zshrc");
    let text = fs::read_to_string(&zshrc).map_err(|e| format!("read ~/.zshrc: {e}"))?;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        if let Some(rest) = line
            .strip_prefix("export ANYROUTER_API_KEY=")
            .or_else(|| line.strip_prefix("ANYROUTER_API_KEY="))
        {
            let key = rest
                .trim()
                .trim_matches('"')
                .trim_matches('\'')
                .to_string();
            if !key.is_empty() {
                return Ok(key);
            }
        }
    }
    Err("ANYROUTER_API_KEY not found in env or ~/.zshrc".into())
}

pub fn fetch_aihub_balance() -> Result<AihubBalance, String> {
    let key = read_anyrouter_key()?;
    let resp = HTTP
        .get("https://aihub.top/v1/usage?days=7")
        .bearer_auth(&key)
        .header("User-Agent", crate::http_util::BROWSER_UA)
        .send()
        .map_err(|e| friendly_http_err("AIHub /v1/usage", e))?;
    if !resp.status().is_success() {
        return Err(format!("AIHub HTTP {}", resp.status()));
    }
    let body: serde_json::Value = resp
        .json()
        .map_err(|e| format!("parse AIHub usage: {e}"))?;

    let balance = body
        .get("balance")
        .or_else(|| body.get("remaining"))
        .and_then(|v| v.as_f64())
        .ok_or_else(|| "AIHub response missing balance".to_string())?;

    let used = body
        .pointer("/usage/today/actual_cost")
        .and_then(|v| v.as_f64())
        .or_else(|| body.pointer("/usage/total/actual_cost").and_then(|v| v.as_f64()))
        .unwrap_or(0.0);

    let currency = body
        .get("currency")
        .and_then(|v| v.as_str())
        .or_else(|| body.get("unit").and_then(|v| v.as_str()))
        .unwrap_or("USD")
        .to_string();

    Ok(AihubBalance {
        balance,
        used,
        currency: if currency.is_empty() {
            "USD".into()
        } else {
            currency.to_uppercase()
        },
        fetched_at: now_iso(),
    })
}

/// Fetch AIHub account remaining balance and used amount.
#[tauri::command]
pub fn get_aihub_balance() -> Result<AihubBalance, String> {
    crate::http_util::cached_json("aihub_balance", Duration::from_secs(60), fetch_aihub_balance)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_balance() {
        let b = fetch_aihub_balance().expect("aihub");
        println!(
            "balance={:.4} used={:.4} {}",
            b.balance, b.used, b.currency
        );
        assert!(b.balance >= 0.0);
    }
}
