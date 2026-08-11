//! Sub2API OpenAI/Codex **OAuth** account-pool quotas (excludes apikey relays).

use crate::gateway::{catalog_path_from_config, codex_config_path, sub2api_dir};
use crate::http_util::{friendly_http_err, now_iso, BROWSER_UA, HTTP};
use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::Duration;
use uuid::Uuid;

const GATEWAY_BASE: &str = "http://127.0.0.1:18080";
const MAX_IMPORT_BYTES: u64 = 10 * 1024 * 1024;

static BROWSER_LOGIN: Mutex<Option<BrowserLoginSession>> = Mutex::new(None);

#[derive(Debug, Clone)]
struct BrowserLoginSession {
    id: String,
    oauth_session_id: String,
    state: String,
    callback_url: Option<String>,
    started_at: DateTime<Utc>,
}

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

/// Sanitized result returned after a local account import.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Sub2ApiImportResult {
    pub created: u32,
    pub updated: u32,
    pub skipped: u32,
    pub failed: u32,
    pub summary: String,
}

/// State of the system-browser OAuth/2FA handoff.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserLoginStatus {
    pub session_id: Option<String>,
    pub login_url: String,
    pub state: String,
    pub message: String,
    pub imported_accounts: Vec<String>,
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

fn admin_post(path: &str, payload: Value) -> Result<Value, String> {
    let token = admin_login()?;
    let resp = HTTP
        .post(format!("{GATEWAY_BASE}{path}"))
        .bearer_auth(&token)
        .header("User-Agent", BROWSER_UA)
        .json(&payload)
        .send()
        .map_err(|e| friendly_http_err(&format!("admin POST {path}"), e))?;
    let status = resp.status();
    let body: Value = resp
        .json()
        .map_err(|e| format!("parse admin {path}: {e}"))?;
    if !status.is_success() {
        let message = body
            .get("message")
            .and_then(Value::as_str)
            .or_else(|| body.pointer("/error/message").and_then(Value::as_str))
            .unwrap_or("request failed");
        return Err(format!("admin {path} HTTP {status}: {message}"));
    }
    Ok(body)
}

fn admin_put(path: &str, payload: Value) -> Result<Value, String> {
    let token = admin_login()?;
    let resp = HTTP
        .put(format!("{GATEWAY_BASE}{path}"))
        .bearer_auth(&token)
        .header("User-Agent", BROWSER_UA)
        .json(&payload)
        .send()
        .map_err(|e| friendly_http_err(&format!("admin PUT {path}"), e))?;
    let status = resp.status();
    let body: Value = resp
        .json()
        .map_err(|e| format!("parse admin {path}: {e}"))?;
    if !status.is_success() {
        let message = body
            .get("message")
            .and_then(Value::as_str)
            .or_else(|| body.pointer("/error/message").and_then(Value::as_str))
            .unwrap_or("request failed");
        return Err(format!("admin PUT {path} HTTP {status}: {message}"));
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

/// Strip a provider prefix from a catalog slug, returning the upstream model
/// id when the remainder looks like a real OpenAI-family model name.
///
/// Catalog slugs are built as `{prefix}-{raw_model}` (see providers.rs), e.g.
/// `sub2api-gpt-5.6-luna` → `gpt-5.6-luna`. Slugs without a recognizable
/// model remainder (e.g. bare `gpt-5.6-sol`) return `None`.
fn strip_provider_prefix(slug: &str) -> Option<String> {
    let (_, rest) = slug.split_once('-')?;
    let looks_like_model = ["gpt", "codex", "claude", "gemini", "grok", "o1", "o3"]
        .iter()
        .any(|needle| rest.contains(needle));
    if looks_like_model {
        Some(rest.to_string())
    } else {
        None
    }
}

/// Collect the `{prefixed_slug -> upstream_model}` mapping an OAuth account
/// needs so Codex catalog model names resolve upstream instead of being sent
/// verbatim (which OpenAI rejects with 400, parking the account → 503).
///
/// Sources, in priority order:
/// 1. `model_mapping` of every apikey relay account (already curated).
/// 2. Prefix-stripped slugs from the active Codex model catalog.
fn known_prefixed_model_mapping() -> Map<String, Value> {
    let mut map = Map::new();

    // 1) Copy mappings from apikey relay accounts.
    if let Ok(body) = admin_get("/api/v1/admin/accounts?page=1&page_size=100") {
        let items = body
            .pointer("/data/items")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        for acc in items {
            if acc.get("type").and_then(|t| t.as_str()) != Some("apikey") {
                continue;
            }
            if let Some(mapping) = acc
                .pointer("/credentials/model_mapping")
                .and_then(|v| v.as_object())
            {
                for (k, v) in mapping {
                    map.entry(k.clone()).or_insert_with(|| v.clone());
                }
            }
        }
    }

    // 2) Derive from the active Codex catalog by stripping provider prefixes.
    if let Ok(raw) = fs::read_to_string(codex_config_path()) {
        if let Ok(doc) = toml::from_str::<toml::Value>(&raw) {
            let catalog_path = catalog_path_from_config(&doc);
            if let Ok(text) = fs::read_to_string(&catalog_path) {
                if let Ok(catalog) = serde_json::from_str::<Value>(&text) {
                    if let Some(models) = catalog.get("models").and_then(|m| m.as_array()) {
                        for model in models {
                            if let Some(slug) = model.get("slug").and_then(|s| s.as_str()) {
                                if let Some(raw_id) = strip_provider_prefix(slug) {
                                    map.entry(slug.to_string())
                                        .or_insert_with(|| Value::String(raw_id));
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    map
}

/// Merge the known prefixed-model mapping into one OAuth account.
/// Returns how many entries were added. Existing keys are never overwritten.
fn ensure_oauth_account_mapping(account_id: i64) -> Result<u32, String> {
    let desired = known_prefixed_model_mapping();
    if desired.is_empty() {
        return Ok(0);
    }
    let body = admin_get(&format!("/api/v1/admin/accounts/{account_id}"))?;
    let acc = body.get("data").cloned().unwrap_or(body);
    if acc.get("type").and_then(|t| t.as_str()) != Some("oauth") {
        return Ok(0);
    }
    let mut mapping = acc
        .pointer("/credentials/model_mapping")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();
    let mut added = 0u32;
    for (key, value) in &desired {
        if !mapping.contains_key(key) {
            mapping.insert(key.clone(), value.clone());
            added += 1;
        }
    }
    if added == 0 {
        return Ok(0);
    }
    admin_put(
        &format!("/api/v1/admin/accounts/{account_id}"),
        json!({ "credentials": { "model_mapping": mapping } }),
    )?;
    Ok(added)
}

/// Best-effort self-heal: make sure every OAuth pool account carries the
/// prefixed-model mapping. Never fails the caller — returns added count.
fn heal_oauth_account_mappings() -> u32 {
    let mut total = 0u32;
    if let Ok(accounts) = list_oauth_accounts() {
        for acc in accounts {
            if let Some(id) = acc.get("id").and_then(|v| v.as_i64()) {
                total += ensure_oauth_account_mapping(id).unwrap_or(0);
            }
        }
    }
    total
}

fn validate_import_path(path: &Path) -> Result<(), String> {

    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !matches!(extension.as_str(), "json" | "jsonl" | "txt") {
        return Err("仅支持 JSON、JSONL 或 TXT 导入文件".into());
    }
    let metadata = fs::metadata(path)
        .map_err(|e| format!("无法读取导入文件 {}: {e}", path.display()))?;
    if !metadata.is_file() {
        return Err("导入路径不是普通文件".into());
    }
    if metadata.len() == 0 {
        return Err("导入文件为空".into());
    }
    if metadata.len() > MAX_IMPORT_BYTES {
        return Err(format!("导入文件超过 {} MiB 限制", MAX_IMPORT_BYTES / 1024 / 1024));
    }
    Ok(())
}

fn private_import_copy(source: &Path) -> Result<PathBuf, String> {
    validate_import_path(source)?;
    let suffix = source
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("json");
    let target = std::env::temp_dir().join(format!("codex-provider-hub-{}.{}", Uuid::new_v4(), suffix));
    fs::copy(source, &target)
        .map_err(|e| format!("创建本地导入副本失败: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&target, fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("保护临时导入文件失败: {e}"))?;
    }
    Ok(target)
}

fn summarize_import_output(output: &str) -> Sub2ApiImportResult {
    let mut result = Sub2ApiImportResult {
        created: 0,
        updated: 0,
        skipped: 0,
        failed: 0,
        summary: "导入已完成，请刷新号池确认账号状态。".into(),
    };
    // The maintained local importer emits these counters for Codex session imports.
    for line in output.lines() {
        let clean = line.trim();
        if clean.contains("access_token") || clean.contains("refresh_token") || clean.contains("Bearer ") {
            continue;
        }
        let count_after = |label: &str| {
            clean
                .split_once(label)
                .and_then(|(_, rest)| {
                    rest.split(|c: char| !c.is_ascii_digit())
                        .find(|part| !part.is_empty())
                })
                .and_then(|part| part.parse::<u32>().ok())
        };
        if let Some(count) = count_after("新增") {
            result.created = count;
        }
        if let Some(count) = count_after("更新") {
            result.updated = count;
        }
        if let Some(count) = count_after("跳过") {
            result.skipped = count;
        }
        if let Some(count) = count_after("失败") {
            result.failed = count;
        }
    }
    result
}

/// Import JSON/JSONL/TXT through the maintained local Sub2API importer.
///
/// Credentials are copied to a 0600 temporary file, never parsed or returned
/// by the Hub, then removed whether the importer succeeds or fails.
#[tauri::command]
pub fn import_sub2api_file(file_path: String, name: Option<String>) -> Result<Sub2ApiImportResult, String> {
    let source = PathBuf::from(file_path);
    let temp_copy = private_import_copy(&source)?;
    let script = sub2api_dir().join("sub2api");
    if !script.is_file() {
        let _ = fs::remove_file(&temp_copy);
        return Err(format!("未找到 Sub2API 导入脚本: {}", script.display()));
    }

    let command_result = Command::new(&script)
        .arg("import")
        .arg(&temp_copy)
        .args(name.as_deref().filter(|value| !value.trim().is_empty()))
        .output();
    let _ = fs::remove_file(&temp_copy);
    let output = command_result.map_err(|e| format!("运行 Sub2API 导入器失败: {e}"))?;
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let result = summarize_import_output(&combined);
    if !output.status.success() {
        let safe_message = combined
            .lines()
            .rev()
            .find(|line| line.contains("[ERROR]") || line.contains("不支持") || line.contains("失败"))
            .map(str::trim)
            .unwrap_or("Sub2API 未接受该导入文件");
        return Err(format!("导入失败: {safe_message}"));
    }
    // Self-heal: imported OAuth accounts need prefixed-model mappings,
    // otherwise OpenAI rejects catalog slugs and the pool 503s.
    let healed = heal_oauth_account_mappings();
    let mut result = result;
    if healed > 0 {
        result.summary = format!("导入完成，并为 OAuth 账号补齐 {healed} 条模型映射。");
    }
    crate::http_util::invalidate_cache("sub2api_usage");
    Ok(result)
}

fn browser_login_url() -> String {
    format!("{GATEWAY_BASE}/admin/accounts")
}

fn start_oauth_callback_listener(session_id: String) -> Result<(), String> {
    let listener = TcpListener::bind("127.0.0.1:1455")
        .map_err(|e| format!("无法监听 OpenAI OAuth 回调端口 1455: {e}"))?;
    thread::spawn(move || {
        for stream in listener.incoming().take(1) {
            let Ok(mut stream) = stream else { return };
            let mut request = [0_u8; 8192];
            let Ok(read) = stream.read(&mut request) else { return };
            let first_line = String::from_utf8_lossy(&request[..read])
                .lines()
                .next()
                .unwrap_or("")
                .to_string();
            let path = first_line
                .split_whitespace()
                .nth(1)
                .filter(|path| path.starts_with("/auth/callback"));
            if let Some(path) = path {
                if let Some(session) = BROWSER_LOGIN.lock().as_mut() {
                    if session.id == session_id {
                        session.callback_url = Some(format!("http://localhost:1455{path}"));
                    }
                }
            }
            let body = "<html><body><h2>Codex Provider Hub</h2><p>登录回调已接收，可回到 Hub 完成导入。</p></body></html>";
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        }
    });
    Ok(())
}

/// Starts a user-driven login handoff. The Hub intentionally opens the local
/// Sub2API account UI rather than collecting an OpenAI password or 2FA code.
#[tauri::command]
pub fn begin_sub2api_browser_login() -> Result<BrowserLoginStatus, String> {
    let response = admin_post("/api/v1/admin/openai/generate-auth-url", json!({}))?;
    let data = response.get("data").unwrap_or(&response);
    let login_url = data
        .get("auth_url")
        .or_else(|| data.get("url"))
        .and_then(Value::as_str)
        .ok_or_else(|| "Sub2API 未返回 OpenAI 授权链接".to_string())?
        .to_string();
    let oauth_session_id = data
        .get("session_id")
        .or_else(|| data.get("sessionId"))
        .and_then(Value::as_str)
        .ok_or_else(|| "Sub2API 未返回 OAuth 会话 ID".to_string())?
        .to_string();
    // Sub2API v0.1.173 returns `auth_url` and `session_id`; `state` is
    // intentionally embedded in the generated authorization URL.
    let state = callback_query_value(&login_url, "state")
        .ok_or_else(|| "授权链接缺少 OAuth state 参数".to_string())?;
    let id = Uuid::new_v4().to_string();
    *BROWSER_LOGIN.lock() = Some(BrowserLoginSession {
        id: id.clone(),
        oauth_session_id,
        state,
        callback_url: None,
        started_at: Utc::now(),
    });
    if let Err(error) = start_oauth_callback_listener(id.clone()) {
        *BROWSER_LOGIN.lock() = None;
        return Err(error);
    }
    Ok(BrowserLoginStatus {
        session_id: Some(id),
        login_url,
        state: "waiting".into(),
        message: "已打开本地 Sub2API 账号页。请新增 OpenAI/Codex OAuth 账号，并在官方浏览器页完成登录、2FA 和验证码。".into(),
        imported_accounts: vec![],
    })
}

#[tauri::command]
pub fn get_sub2api_browser_login_status(session_id: String) -> Result<BrowserLoginStatus, String> {
    let session = BROWSER_LOGIN.lock().clone();
    let Some(session) = session else {
        return Ok(BrowserLoginStatus {
            session_id: None,
            login_url: browser_login_url(),
            state: "cancelled".into(),
            message: "没有进行中的浏览器登录。".into(),
            imported_accounts: vec![],
        });
    };
    if session.id != session_id {
        return Err("浏览器登录会话不匹配".into());
    }
    if (Utc::now() - session.started_at).num_minutes() >= 10 {
        *BROWSER_LOGIN.lock() = None;
        return Ok(BrowserLoginStatus {
            session_id: None,
            login_url: browser_login_url(),
            state: "expired".into(),
            message: "登录等待已超时，请重新开始。".into(),
            imported_accounts: vec![],
        });
    }
    if session.callback_url.is_some() {
        return Ok(BrowserLoginStatus {
            session_id: Some(session.id),
            login_url: browser_login_url(),
            state: "ready".into(),
            message: "已接收浏览器回调，正在导入 OAuth 账号。".into(),
            imported_accounts: vec![],
        });
    }
    Ok(BrowserLoginStatus {
        session_id: Some(session.id),
        login_url: browser_login_url(),
        state: "waiting".into(),
        message: "请在浏览器完成登录后，将最终跳转的完整 URL 粘贴回 Hub。".into(),
        imported_accounts: vec![],
    })
}

fn callback_query_value(callback_url: &str, key: &str) -> Option<String> {
    let query = callback_url.split_once('?')?.1.split('#').next().unwrap_or("");
    query.split('&').find_map(|part| {
        let (name, value) = part.split_once('=')?;
        (name == key).then(|| value.to_string())
    })
}

/// Completes the server-issued OAuth session using the callback URL copied
/// from the system browser after OpenAI login and 2FA.
#[tauri::command]
pub fn complete_sub2api_browser_login(
    session_id: String,
    name: Option<String>,
) -> Result<BrowserLoginStatus, String> {
    let session = BROWSER_LOGIN
        .lock()
        .clone()
        .ok_or_else(|| "没有进行中的浏览器登录。".to_string())?;
    if session.id != session_id {
        return Err("浏览器登录会话不匹配".into());
    }
    let callback_url = session
        .callback_url
        .ok_or_else(|| "尚未收到浏览器 OAuth 回调。".to_string())?;
    let code = callback_query_value(&callback_url, "code")
        .ok_or_else(|| "回调 URL 中没有 OAuth code 参数".to_string())?;
    let state = callback_query_value(&callback_url, "state")
        .ok_or_else(|| "回调 URL 中没有 OAuth state 参数".to_string())?;
    if state != session.state {
        return Err("OAuth state 不匹配，已拒绝完成登录。".into());
    }
    let mut payload = json!({
        "session_id": session.oauth_session_id,
        "code": code,
        "state": state,
    });
    if let Some(name) = name.filter(|value| !value.trim().is_empty()) {
        payload["name"] = Value::String(name);
    }
    let response = admin_post("/api/v1/admin/openai/create-from-oauth", payload)?;
    let data = response.get("data").unwrap_or(&response);
    let account_name = data
        .pointer("/account/name")
        .and_then(Value::as_str)
        .or_else(|| data.get("name").and_then(Value::as_str))
        .unwrap_or("新 OAuth 账号")
        .to_string();
    let account_id = data
        .pointer("/account/id")
        .and_then(Value::as_i64)
        .or_else(|| data.get("id").and_then(Value::as_i64));
    // Without a prefixed-model mapping, OpenAI rejects catalog slugs verbatim
    // (400) and Sub2API parks the account, so the whole pool 503s.
    let mapped = match account_id {
        Some(id) => ensure_oauth_account_mapping(id).unwrap_or(0),
        None => heal_oauth_account_mappings(),
    };
    let message = if mapped > 0 {
        format!("OpenAI/Codex OAuth 账号已导入，并配置 {mapped} 条模型映射。")
    } else {
        "OpenAI/Codex OAuth 账号已导入。".to_string()
    };
    *BROWSER_LOGIN.lock() = None;
    crate::http_util::invalidate_cache("sub2api_usage");
    Ok(BrowserLoginStatus {
        session_id: None,
        login_url: browser_login_url(),
        state: "complete".into(),
        message,
        imported_accounts: vec![account_name],
    })
}

#[tauri::command]
pub fn cancel_sub2api_browser_login(session_id: String) -> Result<(), String> {
    let mut guard = BROWSER_LOGIN.lock();
    if guard.as_ref().is_some_and(|session| session.id == session_id) {
        *guard = None;
        return Ok(());
    }
    Err("浏览器登录会话不匹配或已结束".into())
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
    fn strips_provider_prefix_only_for_model_slugs() {
        assert_eq!(
            strip_provider_prefix("sub2api-gpt-5.6-luna"),
            Some("gpt-5.6-luna".to_string())
        );
        assert_eq!(
            strip_provider_prefix("aihub-claude-opus-5"),
            Some("claude-opus-5".to_string())
        );
        assert_eq!(
            strip_provider_prefix("anyrouter-gpt-5.6-sol"),
            Some("gpt-5.6-sol".to_string())
        );
        // Bare upstream ids and non-model slugs are left alone.
        assert_eq!(strip_provider_prefix("gpt-5.6-sol"), None);
        assert_eq!(strip_provider_prefix("no-dash"), None);
        assert_eq!(strip_provider_prefix("my-relay-name"), None);
    }

    #[test]
    fn accepts_supported_small_import_files_only() {
        let path = std::env::temp_dir().join(format!("hub-import-{}.json", Uuid::new_v4()));
        fs::write(&path, b"{\"refresh_token\":\"test\"}").expect("write fixture");
        assert!(validate_import_path(&path).is_ok());
        let unsupported = path.with_extension("csv");
        fs::rename(&path, &unsupported).expect("rename fixture");
        assert!(validate_import_path(&unsupported).is_err());
        let _ = fs::remove_file(unsupported);
    }

    #[test]
    fn import_summary_does_not_expose_tokens() {
        let result = summarize_import_output(
            "导入完成：总数 3，新增 2，更新 1，跳过 0，失败 0\naccess_token=secret",
        );
        assert_eq!(result.created, 2);
        assert_eq!(result.updated, 1);
        assert_eq!(result.failed, 0);
        assert!(!result.summary.contains("secret"));
    }

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
