//! Cursor multi-account pool: encrypted token storage + usage APIs.

use crate::crypto::{decrypt_secret, encrypt_secret};
use crate::http_util::{friendly_http_err, now_iso, HTTP};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::time::Duration;
use tauri::{AppHandle, Manager};
use uuid::Uuid;

const CURSOR_USAGE_URL: &str =
    "https://api2.cursor.sh/aiserver.v1.DashboardService/GetCurrentPeriodUsage";
const CURSOR_PLAN_URL: &str =
    "https://api2.cursor.sh/aiserver.v1.DashboardService/GetPlanInfo";

/// A Cursor account credential row stored by the hub.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CursorAccount {
    pub id: String,
    pub email: String,
    /// Access token — encrypted at rest; IPC list returns a masked placeholder.
    pub access_token: String,
    pub created_at: String,
}

/// Per-account Cursor plan usage.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CursorUsage {
    pub account_id: String,
    pub email: String,
    pub plan_name: String,
    pub plan_limit: f64,
    pub used: f64,
    pub remaining: f64,
    pub auto_percent: f64,
    pub api_percent: f64,
    pub total_percent: f64,
    pub fetched_at: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CursorStore {
    accounts: Vec<CursorAccount>,
}

fn accounts_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("resolve app data dir: {e}"))?;
    fs::create_dir_all(&dir).map_err(|e| format!("create app data dir: {e}"))?;
    Ok(dir.join("cursor_accounts.json"))
}

fn load_store(app: &AppHandle) -> Result<CursorStore, String> {
    let path = accounts_path(app)?;
    if !path.exists() {
        return Ok(CursorStore::default());
    }
    let raw = fs::read_to_string(&path).map_err(|e| format!("read cursor store: {e}"))?;
    let mut store: CursorStore =
        serde_json::from_str(&raw).map_err(|e| format!("parse cursor store: {e}"))?;

    // Migrate any legacy plaintext tokens to encrypted form.
    let mut dirty = false;
    for account in &mut store.accounts {
        if !crate::crypto::is_encrypted(&account.access_token)
            && !account.access_token.is_empty()
            && !account.access_token.starts_with("••••")
        {
            account.access_token = encrypt_secret(&account.access_token)?;
            dirty = true;
        }
    }
    if dirty {
        save_store(app, &store)?;
    }
    Ok(store)
}

fn save_store(app: &AppHandle, store: &CursorStore) -> Result<(), String> {
    let path = accounts_path(app)?;
    let raw =
        serde_json::to_string_pretty(store).map_err(|e| format!("serialize cursor store: {e}"))?;
    fs::write(&path, raw).map_err(|e| format!("write cursor store: {e}"))
}

fn mask_token(token: &str) -> String {
    if token.len() <= 8 {
        return "••••".into();
    }
    format!("••••…{}", &token[token.len().saturating_sub(4)..])
}

fn for_ipc(account: &CursorAccount) -> CursorAccount {
    let mut a = account.clone();
    // Never send decrypted secrets to the webview.
    a.access_token = mask_token(
        decrypt_secret(&account.access_token)
            .as_deref()
            .unwrap_or("****"),
    );
    a
}

fn plaintext_token(account: &CursorAccount) -> Result<String, String> {
    decrypt_secret(&account.access_token)
}

fn cursor_headers(token: &str) -> reqwest::header::HeaderMap {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::AUTHORIZATION,
        format!("Bearer {token}")
            .parse()
            .unwrap_or(reqwest::header::HeaderValue::from_static("Bearer")),
    );
    headers.insert(
        reqwest::header::CONTENT_TYPE,
        reqwest::header::HeaderValue::from_static("application/json"),
    );
    headers.insert(
        "Connect-Protocol-Version",
        reqwest::header::HeaderValue::from_static("1"),
    );
    headers.insert(
        reqwest::header::USER_AGENT,
        reqwest::header::HeaderValue::from_static(crate::http_util::BROWSER_UA),
    );
    headers
}

fn post_cursor_json(url: &str, token: &str) -> Result<serde_json::Value, String> {
    let resp = HTTP
        .post(url)
        .headers(cursor_headers(token))
        .body("{}")
        .send()
        .map_err(|e| friendly_http_err("Cursor API", e))?;
    let status = resp.status();
    let text = resp.text().map_err(|e| format!("read Cursor body: {e}"))?;
    if status.as_u16() == 401 || status.as_u16() == 403 {
        return Err("token 已失效，请重新登录 Cursor 并更新 accessToken".into());
    }
    if !status.is_success() {
        return Err(format!(
            "Cursor HTTP {status}: {}",
            text.chars().take(120).collect::<String>()
        ));
    }
    serde_json::from_str(&text).map_err(|e| format!("parse Cursor JSON: {e}"))
}

pub fn fetch_cursor_usage_for_token(
    account_id: &str,
    email: &str,
    token: &str,
) -> Result<CursorUsage, String> {
    let usage_body = post_cursor_json(CURSOR_USAGE_URL, token)?;
    let plan_usage = usage_body
        .get("planUsage")
        .cloned()
        .unwrap_or(serde_json::json!({}));

    let included = plan_usage
        .get("includedSpend")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    let limit = plan_usage
        .get("limit")
        .and_then(|v| v.as_f64())
        .unwrap_or(included)
        .max(0.0);
    let remaining_cents = (limit - included).max(0.0);

    let auto_percent = plan_usage
        .get("autoPercentUsed")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    let api_percent = plan_usage
        .get("apiPercentUsed")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    let total_percent = plan_usage
        .get("totalPercentUsed")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);

    let plan_name = match post_cursor_json(CURSOR_PLAN_URL, token) {
        Ok(plan_body) => plan_body
            .pointer("/planInfo/planName")
            .and_then(|v| v.as_str())
            .unwrap_or("Cursor")
            .to_string(),
        Err(_) => "Cursor".into(),
    };

    // API returns cents; surface dollars on the card.
    Ok(CursorUsage {
        account_id: account_id.to_string(),
        email: email.to_string(),
        plan_name,
        plan_limit: limit / 100.0,
        used: included / 100.0,
        remaining: remaining_cents / 100.0,
        auto_percent,
        api_percent,
        total_percent,
        fetched_at: now_iso(),
    })
}

fn validate_token(token: &str) -> Result<(), String> {
    let _ = post_cursor_json(CURSOR_USAGE_URL, token)?;
    Ok(())
}

fn local_cursor_db_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_default()
        .join("Library/Application Support/Cursor/User/globalStorage/state.vscdb")
}

pub fn read_local_cursor_session() -> Result<(String, String), String> {
    let path = local_cursor_db_path();
    if !path.exists() {
        return Err(format!("本机 Cursor 状态库不存在: {}", path.display()));
    }
    let conn = Connection::open_with_flags(
        &path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| format!("open Cursor state.vscdb: {e}"))?;

    let token: String = conn
        .query_row(
            "SELECT value FROM ItemTable WHERE key = ?1",
            ["cursorAuth/accessToken"],
            |row| row.get(0),
        )
        .map_err(|_| "未找到 cursorAuth/accessToken（请先在 Cursor 登录）".to_string())?;
    let email: String = conn
        .query_row(
            "SELECT value FROM ItemTable WHERE key = ?1",
            ["cursorAuth/cachedEmail"],
            |row| row.get(0),
        )
        .unwrap_or_else(|_| "unknown@cursor.local".into());

    let token = token.trim().to_string();
    let email = email.trim().to_string();
    if token.is_empty() {
        return Err("本机 accessToken 为空".into());
    }
    Ok((email, token))
}

/// List persisted Cursor accounts (tokens masked for IPC).
#[tauri::command]
pub fn list_cursor_accounts(app: AppHandle) -> Result<Vec<CursorAccount>, String> {
    Ok(load_store(&app)?.accounts.iter().map(for_ipc).collect())
}

/// Add a Cursor account (`email` + `accessToken`) after validating the token.
#[tauri::command]
pub fn add_cursor_account(
    app: AppHandle,
    email: String,
    access_token: String,
) -> Result<CursorAccount, String> {
    let mut email = email.trim().to_string();
    let access_token = access_token.trim().to_string();
    if access_token.is_empty() {
        return Err("accessToken is required".into());
    }
    validate_token(&access_token)?;
    if email.is_empty() {
        email = "cursor-user".into();
    }

    let mut store = load_store(&app)?;
    if let Some(idx) = store
        .accounts
        .iter()
        .position(|a| a.email.eq_ignore_ascii_case(&email))
    {
        store.accounts[idx].access_token = encrypt_secret(&access_token)?;
        save_store(&app, &store)?;
        return Ok(for_ipc(&store.accounts[idx]));
    }

    let account = CursorAccount {
        id: Uuid::new_v4().to_string(),
        email,
        access_token: encrypt_secret(&access_token)?,
        created_at: now_iso(),
    };
    store.accounts.push(account.clone());
    save_store(&app, &store)?;
    Ok(for_ipc(&account))
}

/// Import the currently logged-in Cursor account from local state.vscdb.
#[tauri::command]
pub fn import_local_cursor_account(app: AppHandle) -> Result<CursorAccount, String> {
    let (email, token) = read_local_cursor_session()?;
    add_cursor_account(app, email, token)
}

/// Remove a Cursor account by id from the local pool.
#[tauri::command]
pub fn remove_cursor_account(app: AppHandle, id: String) -> Result<(), String> {
    let mut store = load_store(&app)?;
    let before = store.accounts.len();
    store.accounts.retain(|a| a.id != id);
    if store.accounts.len() == before {
        return Err(format!("account not found: {id}"));
    }
    save_store(&app, &store)?;
    Ok(())
}

/// Fetch plan usage for one Cursor account.
#[tauri::command]
pub fn get_cursor_usage(app: AppHandle, id: String) -> Result<CursorUsage, String> {
    let store = load_store(&app)?;
    let account = store
        .accounts
        .iter()
        .find(|a| a.id == id)
        .cloned()
        .ok_or_else(|| format!("account not found: {id}"))?;
    let token = plaintext_token(&account)?;
    let cache_key = format!("cursor_usage_{id}");
    crate::http_util::cached_json(&cache_key, Duration::from_secs(60), || {
        fetch_cursor_usage_for_token(&account.id, &account.email, &token)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_local_session_usage() {
        let (email, token) = read_local_cursor_session().expect("local session");
        println!("email={email} token_len={}", token.len());
        let usage = fetch_cursor_usage_for_token("test", &email, &token).expect("usage");
        println!(
            "plan={} used={:.2}/{:.2} total%={:.1} auto%={:.1} api%={:.1}",
            usage.plan_name,
            usage.used,
            usage.plan_limit,
            usage.total_percent,
            usage.auto_percent,
            usage.api_percent
        );
        assert!(!usage.plan_name.is_empty());
    }
}
