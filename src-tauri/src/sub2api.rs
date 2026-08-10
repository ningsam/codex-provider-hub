//! Sub2API OpenAI/Codex **OAuth** account-pool quotas (excludes apikey relays).

use crate::gateway::sub2api_dir;
use crate::http_util::{friendly_http_err, now_iso, BROWSER_UA, HTTP};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::time::Duration;

const GATEWAY_BASE: &str = "http://127.0.0.1:18080";

/// Remaining quota window for Sub2API (5h or 7d).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaWindow {
    pub remaining_percent: f64,
    pub reset_after_seconds: u64,
}

/// One real OpenAI/Codex OAuth account in the pool.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Sub2ApiAccountQuota {
    pub id: i64,
    pub name: String,
    pub email: String,
    /// Normalized: `ready` | `error` | `inactive` | other raw status.
    pub status: String,
    pub error_message: String,
    pub five_hour: Option<QuotaWindow>,
    pub seven_day: Option<QuotaWindow>,
    pub schedulable: bool,
}

/// Aggregated + per-account Sub2API OAuth pool snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Sub2ApiUsage {
    /// Average across OAuth accounts that still report a 5h window (includes errored if known).
    pub five_hour: QuotaWindow,
    pub seven_day: QuotaWindow,
    /// OAuth accounts only (apikey relays like AIHub/AnyRouter are excluded).
    pub pool_total: u32,
    pub pool_available: u32,
    pub accounts: Vec<Sub2ApiAccountQuota>,
    pub fetched_at: String,
}

fn read_admin_creds() -> Result<(String, String), String> {
    let env_path = sub2api_dir().join(".env");
    let text = fs::read_to_string(&env_path)
        .map_err(|e| format!("read {}: {e}", env_path.display()))?;
    let mut email = String::new();
    let mut password = String::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(v) = line.strip_prefix("ADMIN_EMAIL=") {
            email = v.trim().trim_matches('"').trim_matches('\'').to_string();
        } else if let Some(v) = line.strip_prefix("ADMIN_PASSWORD=") {
            password = v.trim().trim_matches('"').trim_matches('\'').to_string();
        }
    }
    if email.is_empty() || password.is_empty() {
        return Err("ADMIN_EMAIL / ADMIN_PASSWORD missing in Sub2API .env".into());
    }
    Ok((email, password))
}

fn admin_login() -> Result<String, String> {
    let (email, password) = read_admin_creds()?;
    let resp = HTTP
        .post(format!("{GATEWAY_BASE}/api/v1/auth/login"))
        .json(&json!({ "email": email, "password": password }))
        .send()
        .map_err(|e| friendly_http_err("admin login", e))?;
    if !resp.status().is_success() {
        return Err(format!("admin login HTTP {}", resp.status()));
    }
    let body: Value = resp
        .json()
        .map_err(|e| format!("parse admin login: {e}"))?;
    body.pointer("/data/access_token")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "admin login missing access_token".into())
}

fn admin_get(path: &str) -> Result<Value, String> {
    let token = admin_login()?;
    let resp = HTTP
        .get(format!("{GATEWAY_BASE}{path}"))
        .bearer_auth(&token)
        .header("User-Agent", BROWSER_UA)
        .send()
        .map_err(|e| friendly_http_err(&format!("admin {path}"), e))?;
    let status = resp.status();
    let body: Value = resp
        .json()
        .map_err(|e| format!("parse admin {path}: {e}"))?;
    if !status.is_success() {
        let msg = body
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("request failed");
        return Err(format!("admin {path} HTTP {status}: {msg}"));
    }
    Ok(body)
}

fn admin_delete(path: &str) -> Result<(), String> {
    let token = admin_login()?;
    let resp = HTTP
        .delete(format!("{GATEWAY_BASE}{path}"))
        .bearer_auth(&token)
        .header("User-Agent", BROWSER_UA)
        .send()
        .map_err(|e| friendly_http_err(&format!("admin DELETE {path}"), e))?;
    let status = resp.status();
    if status.is_success() || status.as_u16() == 204 {
        return Ok(());
    }
    let body: Value = resp.json().unwrap_or(json!({}));
    let msg = body
        .get("message")
        .and_then(|m| m.as_str())
        .or_else(|| body.pointer("/error/message").and_then(|m| m.as_str()))
        .unwrap_or("request failed");
    Err(format!("admin DELETE {path} HTTP {status}: {msg}"))
}

fn window_from_usage(node: &Value) -> Option<QuotaWindow> {
    if node.is_null() {
        return None;
    }
    let utilization = node
        .get("utilization")
        .and_then(|v| v.as_f64())
        .or_else(|| {
            // some payloads use used_percent
            node.get("used_percent").and_then(|v| v.as_f64())
        })?;
    let remaining = (100.0 - utilization).clamp(0.0, 100.0);
    let reset = node
        .get("remaining_seconds")
        .and_then(|v| v.as_u64())
        .or_else(|| node.get("reset_after_seconds").and_then(|v| v.as_u64()))
        .unwrap_or(0);
    Some(QuotaWindow {
        remaining_percent: remaining,
        reset_after_seconds: reset,
    })
}

fn normalize_status(raw: &str, error_message: &str) -> String {
    let s = raw.trim().to_lowercase();
    if !error_message.trim().is_empty()
        || s == "error"
        || s.contains("error")
        || s.contains("banned")
        || s.contains("revoked")
    {
        return "error".into();
    }
    if s == "active" || s == "ready" || s == "normal" {
        return "ready".into();
    }
    if s == "inactive" || s == "disabled" || s == "paused" {
        return "inactive".into();
    }
    if s.is_empty() {
        "unknown".into()
    } else {
        s
    }
}

fn avg_window(windows: &[QuotaWindow]) -> QuotaWindow {
    if windows.is_empty() {
        return QuotaWindow {
            remaining_percent: 0.0,
            reset_after_seconds: 0,
        };
    }
    let n = windows.len() as f64;
    let remaining = windows.iter().map(|w| w.remaining_percent).sum::<f64>() / n;
    let reset = windows
        .iter()
        .map(|w| w.reset_after_seconds)
        .min()
        .unwrap_or(0);
    QuotaWindow {
        remaining_percent: remaining,
        reset_after_seconds: reset,
    }
}

fn fetch_account_usage(id: i64) -> (Option<QuotaWindow>, Option<QuotaWindow>) {
    match admin_get(&format!("/api/v1/admin/accounts/{id}/usage")) {
        Ok(body) => {
            let data = body.get("data").cloned().unwrap_or(body);
            (
                window_from_usage(data.get("five_hour").unwrap_or(&Value::Null)),
                window_from_usage(data.get("seven_day").unwrap_or(&Value::Null)),
            )
        }
        Err(_) => (None, None),
    }
}

fn list_oauth_accounts() -> Result<Vec<Value>, String> {
    let mut out = Vec::new();
    let mut page = 1u32;
    loop {
        let body = admin_get(&format!(
            "/api/v1/admin/accounts?page={page}&page_size=50&type=oauth"
        ))?;
        let items = body
            .pointer("/data/items")
            .and_then(|v| v.as_array())
            .cloned()
            .or_else(|| {
                body.get("data")
                    .and_then(|d| d.as_array())
                    .cloned()
            })
            .unwrap_or_default();
        let n = items.len();
        out.extend(items);
        if n < 50 {
            break;
        }
        page += 1;
        if page > 20 {
            break;
        }
    }
    // Fallback: some builds ignore type= filter — filter client-side.
    if out.is_empty() {
        let body = admin_get("/api/v1/admin/accounts?page=1&page_size=100")?;
        let items = body
            .pointer("/data/items")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        out = items
            .into_iter()
            .filter(|a| a.get("type").and_then(|t| t.as_str()) == Some("oauth"))
            .collect();
    }
    Ok(out)
}

pub fn fetch_sub2api_usage() -> Result<Sub2ApiUsage, String> {
    let raw_accounts = list_oauth_accounts()?;
    let mut accounts: Vec<Sub2ApiAccountQuota> = Vec::with_capacity(raw_accounts.len());

    for a in raw_accounts {
        let id = a.get("id").and_then(|v| v.as_i64()).unwrap_or(0);
        let name = a
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("unnamed")
            .to_string();
        let email = a
            .pointer("/credentials/email")
            .and_then(|v| v.as_str())
            .or_else(|| a.get("email").and_then(|v| v.as_str()))
            .unwrap_or("")
            .to_string();
        let raw_status = a.get("status").and_then(|v| v.as_str()).unwrap_or("");
        let error_message = a
            .get("error_message")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let status = normalize_status(raw_status, &error_message);
        let schedulable = a
            .get("schedulable")
            .and_then(|v| v.as_bool())
            .unwrap_or(status == "ready");

        let (five_hour, seven_day) = if id > 0 {
            fetch_account_usage(id)
        } else {
            (None, None)
        };

        // Prefer usage endpoint; fall back to account.extra codex_* fields if present.
        let five_hour = five_hour.or_else(|| {
            let used = a
                .pointer("/extra/codex_5h_used_percent")
                .and_then(|v| v.as_f64())?;
            Some(QuotaWindow {
                remaining_percent: (100.0 - used).clamp(0.0, 100.0),
                reset_after_seconds: 0,
            })
        });
        let seven_day = seven_day.or_else(|| {
            let used = a
                .pointer("/extra/codex_7d_used_percent")
                .and_then(|v| v.as_f64())?;
            Some(QuotaWindow {
                remaining_percent: (100.0 - used).clamp(0.0, 100.0),
                reset_after_seconds: 0,
            })
        });

        accounts.push(Sub2ApiAccountQuota {
            id,
            name,
            email,
            status,
            error_message,
            five_hour,
            seven_day,
            schedulable,
        });
    }

    accounts.sort_by(|a, b| {
        // ready first, then by name
        let rank = |s: &str| match s {
            "ready" => 0,
            "inactive" => 1,
            "error" => 2,
            _ => 3,
        };
        rank(&a.status)
            .cmp(&rank(&b.status))
            .then(a.name.cmp(&b.name))
    });

    let pool_total = accounts.len() as u32;
    let pool_available = accounts.iter().filter(|a| a.status == "ready").count() as u32;

    let five_windows: Vec<_> = accounts
        .iter()
        .filter_map(|a| a.five_hour.clone())
        .collect();
    let seven_windows: Vec<_> = accounts
        .iter()
        .filter_map(|a| a.seven_day.clone())
        .collect();

    Ok(Sub2ApiUsage {
        five_hour: avg_window(&five_windows),
        seven_day: avg_window(&seven_windows),
        pool_total,
        pool_available,
        accounts,
        fetched_at: now_iso(),
    })
}

/// Fetch per-account OpenAI/Codex OAuth quotas (excludes apikey 中转站).
#[tauri::command]
pub fn get_sub2api_usage() -> Result<Sub2ApiUsage, String> {
    crate::http_util::cached_json("sub2api_usage", Duration::from_secs(30), fetch_sub2api_usage)
}

/// Delete one OAuth account from the Sub2API pool.
///
/// Only `type=oauth` is allowed — apikey relays (AIHub/AnyRouter) must be
/// removed via the Providers card.
#[tauri::command]
pub fn delete_sub2api_account(account_id: i64) -> Result<(), String> {
    if account_id <= 0 {
        return Err("无效的 account_id".into());
    }
    let body = admin_get(&format!("/api/v1/admin/accounts/{account_id}"))?;
    let acc = body.get("data").cloned().unwrap_or(body);
    let acc_type = acc
        .get("type")
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if acc_type != "oauth" {
        return Err(format!(
            "只能删除 type=oauth 的号池账号（当前 type={acc_type}）。AIHub/AnyRouter 请在「供应商」卡删除。"
        ));
    }
    let name = acc
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if name.eq_ignore_ascii_case("AIHub") || name.eq_ignore_ascii_case("AnyRouter") {
        return Err("拒绝删除 AIHub/AnyRouter（中转站请走供应商卡）".into());
    }

    admin_delete(&format!("/api/v1/admin/accounts/{account_id}"))?;
    crate::http_util::invalidate_cache("sub2api_usage");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_oauth_accounts() {
        let u = fetch_sub2api_usage().expect("usage");
        println!(
            "oauth pool {}/{} accounts={}",
            u.pool_available,
            u.pool_total,
            u.accounts.len()
        );
        for a in &u.accounts {
            println!(
                "  #{} {} status={} err={} 5h={:?} 7d={:?}",
                a.id,
                a.name,
                a.status,
                a.error_message.chars().take(60).collect::<String>(),
                a.five_hour.as_ref().map(|w| w.remaining_percent),
                a.seven_day.as_ref().map(|w| w.remaining_percent),
            );
        }
        // Relays must not be counted as oauth pool members.
        assert!(
            u.accounts
                .iter()
                .all(|a| !a.name.eq_ignore_ascii_case("AIHub")
                    && !a.name.eq_ignore_ascii_case("AnyRouter"))
        );
    }

    #[test]
    fn delete_oauth_only_and_banned() {
        // Refuse apikey relays (AIHub / AnyRouter).
        let err = delete_sub2api_account(2).expect_err("must refuse AIHub");
        assert!(
            err.contains("oauth") || err.contains("AIHub") || err.contains("供应商"),
            "{err}"
        );
        let err = delete_sub2api_account(3).expect_err("must refuse AnyRouter");
        assert!(
            err.contains("oauth") || err.contains("AnyRouter") || err.contains("供应商"),
            "{err}"
        );

        let before = fetch_sub2api_usage().expect("usage");
        let banned = before.accounts.iter().find(|a| a.id == 1).cloned();
        if let Some(a) = banned {
            delete_sub2api_account(a.id).expect("delete banned oauth");
            let after = fetch_sub2api_usage().expect("usage after");
            assert!(
                !after.accounts.iter().any(|x| x.id == a.id),
                "account #{} still listed",
                a.id
            );
            println!("deleted oauth #{} {}", a.id, a.name);
        } else {
            println!("oauth #1 already gone — skip delete");
        }
    }
}
