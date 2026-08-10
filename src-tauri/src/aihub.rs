//! AIHub (中转站) wallet balance.
//!
//! Key resolution order:
//! 1. App-data encrypted key (manual UI / last successful Sub2API sync)
//! 2. Sub2API Postgres `accounts` row named AIHub (`credentials.api_key`)
//! 3. `ANYROUTER_API_KEY` from env / ~/.zshrc
//! 4. Local gateway `/v1/usage` for aihub-* today cost (balance still prefers aihub.top)

use crate::crypto::{decrypt_secret, encrypt_secret};
use crate::gateway::{read_gateway_key, sub2api_dir};
use crate::http_util::{friendly_http_err, now_iso, HTTP};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

const AIHUB_USAGE_URL: &str = "https://aihub.top/v1/usage?days=7";
const GATEWAY_USAGE_URL: &str = "http://127.0.0.1:18080/v1/usage?days=7";

/// AIHub wallet / usage balance snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AihubBalance {
    pub balance: f64,
    pub used: f64,
    pub currency: String,
    pub fetched_at: String,
    /// Which credential source produced this snapshot.
    pub key_source: String,
    /// Whether an encrypted key is persisted in app data.
    pub has_stored_key: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AihubKeyStore {
    /// Encrypted API key (`enc:v1:…`).
    api_key: String,
    /// `manual` | `sub2api`
    source: String,
}

fn app_data_dir() -> PathBuf {
    dirs::data_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("com.skylerenzi.codex-provider-hub")
}

fn key_store_path() -> PathBuf {
    app_data_dir().join("aihub_api_key.json")
}

fn load_key_store() -> Option<AihubKeyStore> {
    let path = key_store_path();
    let raw = fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

fn save_key_store(store: &AihubKeyStore) -> Result<(), String> {
    let dir = app_data_dir();
    fs::create_dir_all(&dir).map_err(|e| format!("create app data dir: {e}"))?;
    let path = key_store_path();
    let raw =
        serde_json::to_string_pretty(store).map_err(|e| format!("serialize aihub key: {e}"))?;
    fs::write(&path, raw).map_err(|e| format!("write aihub key: {e}"))
}

fn read_persisted_key() -> Result<Option<(String, String)>, String> {
    let Some(store) = load_key_store() else {
        return Ok(None);
    };
    if store.api_key.trim().is_empty() {
        return Ok(None);
    }
    let plain = decrypt_secret(&store.api_key)?;
    if plain.trim().is_empty() {
        return Ok(None);
    }
    let label = if store.source == "manual" {
        "app-data (手动设置)".to_string()
    } else {
        "app-data (Sub2API 同步)".to_string()
    };
    Ok(Some((plain, label)))
}

fn persist_key(plaintext: &str, source: &str) -> Result<(), String> {
    let store = AihubKeyStore {
        api_key: encrypt_secret(plaintext.trim())?,
        source: source.to_string(),
    };
    save_key_store(&store)
}

fn read_env_file_kv(path: &PathBuf, key: &str) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(rest) = line.strip_prefix(&format!("{key}=")) {
            let v = rest.trim().trim_matches('"').trim_matches('\'').to_string();
            if !v.is_empty() {
                return Some(v);
            }
        }
    }
    None
}

/// Read plaintext AIHub api_key from Sub2API Postgres via docker-compose.
fn read_sub2api_aihub_key() -> Result<String, String> {
    let dir = sub2api_dir();
    let env_path = dir.join(".env");
    let user = read_env_file_kv(&env_path, "POSTGRES_USER").unwrap_or_else(|| "sub2api".into());
    let db = read_env_file_kv(&env_path, "POSTGRES_DB").unwrap_or_else(|| "sub2api".into());
    let sql = "SELECT credentials->>'api_key' FROM accounts \
               WHERE lower(name)='aihub' AND type='apikey' AND deleted_at IS NULL \
               ORDER BY id LIMIT 1;";
    let output = Command::new("docker-compose")
        .args([
            "-f",
            "compose.yaml",
            "exec",
            "-T",
            "postgres",
            "psql",
            "-U",
            &user,
            "-d",
            &db,
            "-t",
            "-A",
            "-c",
            sql,
        ])
        .current_dir(&dir)
        .output()
        .map_err(|e| format!("docker-compose exec postgres: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "读取 Sub2API AIHub credentials 失败: {}",
            stderr.trim()
        ));
    }
    let key = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if key.is_empty() || key == "null" {
        return Err("Sub2API 中未找到名为 AIHub 的 apikey 账号".into());
    }
    Ok(key)
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

fn parse_aihub_usage_body(body: &Value, key_source: &str, has_stored_key: bool) -> Result<AihubBalance, String> {
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
        key_source: key_source.to_string(),
        has_stored_key,
    })
}

fn fetch_aihub_with_key(key: &str, source: &str, has_stored_key: bool) -> Result<AihubBalance, String> {
    let resp = HTTP
        .get(AIHUB_USAGE_URL)
        .bearer_auth(key)
        .header("User-Agent", crate::http_util::BROWSER_UA)
        .send()
        .map_err(|e| friendly_http_err(&format!("AIHub /v1/usage ({source})"), e))?;
    let status = resp.status();
    if status.as_u16() == 401 {
        return Err(format!(
            "AIHub HTTP 401（源: {source}）— API key 无效或已失效"
        ));
    }
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        let snippet = body.chars().take(120).collect::<String>();
        return Err(format!(
            "AIHub HTTP {status}（源: {source}）{snippet}"
        ));
    }
    let body: Value = resp
        .json()
        .map_err(|e| format!("parse AIHub usage ({source}): {e}"))?;
    parse_aihub_usage_body(&body, source, has_stored_key)
}

/// Gateway fallback: sum today's aihub-* model costs. Balance is unavailable here.
fn fetch_gateway_aihub_partial(has_stored_key: bool) -> Result<AihubBalance, String> {
    let gw_key = read_gateway_key()?;
    let resp = HTTP
        .get(GATEWAY_USAGE_URL)
        .bearer_auth(&gw_key)
        .header("User-Agent", crate::http_util::BROWSER_UA)
        .send()
        .map_err(|e| friendly_http_err("gateway /v1/usage", e))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("gateway /v1/usage HTTP {status}（源: local gateway）"));
    }
    let body: Value = resp
        .json()
        .map_err(|e| format!("parse gateway usage: {e}"))?;

    let used = body
        .get("model_stats")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter(|m| {
                    m.get("model")
                        .and_then(|s| s.as_str())
                        .map(|s| s.starts_with("aihub-"))
                        .unwrap_or(false)
                })
                .map(|m| {
                    m.get("actual_cost")
                        .and_then(|v| v.as_f64())
                        .or_else(|| m.get("cost").and_then(|v| v.as_f64()))
                        .unwrap_or(0.0)
                })
                .sum::<f64>()
        })
        .unwrap_or(0.0);

    // Prefer today's daily_usage actual_cost when model_stats is window-wide.
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let today_total = body
        .get("daily_usage")
        .and_then(|v| v.as_array())
        .and_then(|arr| {
            arr.iter()
                .find(|d| d.get("date").and_then(|x| x.as_str()) == Some(today.as_str()))
                .and_then(|d| d.get("actual_cost").and_then(|v| v.as_f64()))
        });

    // model_stats on gateway may be multi-day; use today daily as soft cap hint only when
    // aihub share unknown. Prefer filtered model_stats if non-zero.
    let used = if used > 0.0 {
        used
    } else {
        today_total.unwrap_or(0.0)
    };

    Ok(AihubBalance {
        balance: 0.0,
        used,
        currency: "USD".into(),
        fetched_at: now_iso(),
        key_source: "local gateway（仅今日 aihub-* 消耗；余额需有效 AIHub key）".into(),
        has_stored_key,
    })
}

pub fn fetch_aihub_balance() -> Result<AihubBalance, String> {
    let has_stored_key = load_key_store()
        .map(|s| !s.api_key.trim().is_empty())
        .unwrap_or(false);
    let mut errors: Vec<String> = Vec::new();

    // 1) Persisted app-data key
    match read_persisted_key() {
        Ok(Some((key, label))) => match fetch_aihub_with_key(&key, &label, has_stored_key) {
            Ok(b) => return Ok(b),
            Err(e) => errors.push(e),
        },
        Ok(None) => {}
        Err(e) => errors.push(format!("app-data key: {e}")),
    }

    // 2) Sub2API AIHub account (postgres credentials)
    match read_sub2api_aihub_key() {
        Ok(key) => match fetch_aihub_with_key(&key, "Sub2API AIHub account", has_stored_key) {
            Ok(b) => {
                // Cache for next tray/UI refresh (encrypted).
                let _ = persist_key(&key, "sub2api");
                let mut b = b;
                b.has_stored_key = true;
                b.key_source = "Sub2API AIHub account".into();
                return Ok(b);
            }
            Err(e) => errors.push(e),
        },
        Err(e) => errors.push(e),
    }

    // 3) ANYROUTER_API_KEY (often actually AnyRouter — may 401 on aihub.top)
    match read_anyrouter_key() {
        Ok(key) => {
            match fetch_aihub_with_key(&key, "ANYROUTER_API_KEY (env/zshrc)", has_stored_key) {
                Ok(b) => return Ok(b),
                Err(e) => errors.push(e),
            }
        }
        Err(e) => errors.push(e),
    }

    // 4) Gateway partial fallback (used only)
    match fetch_gateway_aihub_partial(has_stored_key) {
        Ok(b) => {
            if !errors.is_empty() {
                // Still return partial data so the card is not empty, but surface key failures
                // via key_source.
                let mut b = b;
                b.key_source = format!(
                    "{} · aihub.top 失败: {}",
                    b.key_source,
                    errors.join(" | ")
                );
                return Ok(b);
            }
            return Ok(b);
        }
        Err(e) => errors.push(e),
    }

    Err(format!("AIHub 余额不可用: {}", errors.join(" | ")))
}

/// Fetch AIHub account remaining balance and used amount.
#[tauri::command]
pub fn get_aihub_balance() -> Result<AihubBalance, String> {
    crate::http_util::cached_json("aihub_balance", Duration::from_secs(60), fetch_aihub_balance)
}

/// Persist an AIHub API key (encrypted) for subsequent balance fetches.
#[tauri::command]
pub fn set_aihub_api_key(api_key: String) -> Result<AihubBalance, String> {
    let key = api_key.trim().to_string();
    if key.is_empty() {
        return Err("API key 不能为空".into());
    }
    // Verify before saving.
    let bal = fetch_aihub_with_key(&key, "app-data (手动设置)", true)?;
    persist_key(&key, "manual")?;
    crate::http_util::invalidate_cache("aihub_balance");
    Ok(bal)
}

/// Remove the persisted AIHub API key (falls back to Sub2API / env).
#[tauri::command]
pub fn clear_aihub_api_key() -> Result<(), String> {
    let path = key_store_path();
    if path.exists() {
        fs::remove_file(&path).map_err(|e| format!("remove aihub key: {e}"))?;
    }
    crate::http_util::invalidate_cache("aihub_balance");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_balance() {
        let b = fetch_aihub_balance().expect("aihub");
        println!(
            "balance={:.4} used={:.4} {} source={}",
            b.balance, b.used, b.currency, b.key_source
        );
        assert!(b.balance >= 0.0);
        // Prefer a successful aihub.top path (not gateway-only).
        assert!(
            !b.key_source.contains("仅今日"),
            "expected aihub.top key source, got {}",
            b.key_source
        );
    }

    #[test]
    fn sub2api_key_readable() {
        let key = read_sub2api_aihub_key().expect("sub2api aihub key");
        assert!(key.starts_with("sk-"));
        let b = fetch_aihub_with_key(&key, "test", false).expect("usage");
        assert!(b.balance > 0.0);
    }
}
