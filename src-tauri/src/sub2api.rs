//! Sub2API OpenAI/Codex **OAuth** account-pool quotas (excludes apikey relays).

use crate::gateway::{catalog_path_from_config, codex_config_path, read_gateway_key, sub2api_dir};
use crate::http_util::{friendly_http_err, now_iso, BROWSER_UA, HTTP};
use chrono::{DateTime, Utc};
use once_cell::sync::OnceCell;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use uuid::Uuid;

const GATEWAY_BASE: &str = "http://127.0.0.1:18080";
const MAX_IMPORT_BYTES: u64 = 10 * 1024 * 1024;
const PREFERRED_OAUTH_GROUP_PREFIX: &str = "codex-provider-hub:preferred-oauth:";
const AUTOMATIC_GROUP_PREFIX: &str = "codex-provider-hub:auto:";
// Sub2API evaluates this setting from its cached `extra.codex_*` snapshot,
// not from an authoritative wham/usage request. Keep the upstream-safe default
// disabled; users can explicitly opt in to a lower threshold from the UI.
const AUTO_PAUSE_DEFAULT_PERCENT: u8 = 100;
const AUTO_PAUSE_SENTINEL: &str = "hub-openai-auto-pause-default-v1";
const ROUTING_POLICY_FILE: &str = "hub-routing-policy-v1";
const ROUTING_PREFERRED_ACCOUNT_FILE: &str = "hub-routing-preferred-account-v1";
const RECENT_ROUTE_WINDOW_MINUTES: i64 = 10;
const RECENT_ROUTE_REQUEST_LIMIT: u32 = 100;
const ADMIN_TOKEN_TTL: Duration = Duration::from_secs(5 * 60);
const ADMIN_LOGIN_BACKOFF: Duration = Duration::from_secs(2);
const ADMIN_LOGIN_RATE_LIMIT_BACKOFF: Duration = Duration::from_secs(10);

static BROWSER_LOGIN: Mutex<Option<BrowserLoginSession>> = Mutex::new(None);
static BROWSER_CALLBACK_LISTENER: OnceCell<()> = OnceCell::new();
static ROUTING_MUTATION: Mutex<()> = Mutex::new(());
static AUTO_PAUSE_INITIALIZATION: Mutex<()> = Mutex::new(());
static ADMIN_AUTH: Mutex<AdminAuthState> = Mutex::new(AdminAuthState::Empty);

#[derive(Clone)]
struct CachedAdminToken {
    value: String,
    expires_at: Instant,
}

enum AdminAuthState {
    Empty,
    Ready(CachedAdminToken),
    Backoff { retry_at: Instant, error: String },
}

impl AdminAuthState {
    fn cached_result_at(&self, now: Instant) -> Option<Result<String, String>> {
        match self {
            Self::Ready(token) if token.expires_at > now => Some(Ok(token.value.clone())),
            Self::Backoff { retry_at, error } if *retry_at > now => Some(Err(error.clone())),
            _ => None,
        }
    }
}

struct AdminLoginError {
    message: String,
    retry_after: Duration,
}

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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OfficialQuotaWindow {
    pub used_percent: f64,
    pub limit_reached: bool,
    pub reset_after_seconds: u64,
    pub reset_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OfficialQuotaProbe {
    pub account_id: i64,
    pub plan_type: String,
    pub allowed: bool,
    pub limit_reached: bool,
    pub five_hour: Option<OfficialQuotaWindow>,
    pub seven_day: Option<OfficialQuotaWindow>,
    pub fetched_at: String,
}

/// One real OpenAI/Codex OAuth account in the pool.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Sub2ApiAccountQuota {
    pub id: i64,
    pub name: String,
    pub email: String,
    /// oauth | relay | apikey | …
    pub account_type: String,
    /// Normalized: `ready` | `error` | `inactive` | other raw status.
    pub status: String,
    pub error_message: String,
    pub five_hour: Option<QuotaWindow>,
    pub seven_day: Option<QuotaWindow>,
    pub schedulable: bool,
    pub available: bool,
    pub availability: String,
    pub availability_reason: String,
    pub recoverable: bool,
    pub unavailable_until: Option<String>,
    pub preferred: bool,
}

/// Hub preference and Sub2API failover state for the current gateway key.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Sub2ApiRoutingStatus {
    pub preferred_account_id: Option<i64>,
    pub state: String,
    pub message: String,
    pub auto_pause_threshold_percent: u8,
    pub policy: String,
    pub policy_configured: bool,
    pub recent_window_minutes: u32,
    pub recent_request_limit: u32,
    pub recent_request_count: u32,
    pub last_successful_account_id: Option<i64>,
    pub last_successful_account_name: Option<String>,
    pub last_successful_account_type: Option<String>,
    pub last_successful_at: Option<String>,
    pub distribution: Vec<Sub2ApiRoutingDistribution>,
    pub oauth_available_count: u32,
    pub relay_available_count: u32,
    pub policy_deviation: bool,
    pub policy_deviation_message: Option<String>,
    pub active_relay_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Sub2ApiRoutingDistribution {
    pub account_id: i64,
    pub name: String,
    pub account_type: String,
    pub request_count: u32,
    pub percent: f64,
}

/// Aggregated + per-account Sub2API OAuth pool snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Sub2ApiUsage {
    /// Average across OAuth accounts with an active 5h window; `None` when no
    /// account reports one (upstream simply has no such window).
    pub five_hour: Option<QuotaWindow>,
    pub seven_day: Option<QuotaWindow>,
    /// OAuth accounts only (apikey relays like AIHub/AnyRouter are excluded).
    pub pool_total: u32,
    pub pool_available: u32,
    pub accounts: Vec<Sub2ApiAccountQuota>,
    pub routing: Sub2ApiRoutingStatus,
    pub fetched_at: String,
}

#[derive(Debug, Clone)]
struct AccountAvailability {
    available: bool,
    availability: String,
    reason: String,
    recoverable: bool,
    unavailable_until: Option<String>,
}

#[derive(Debug, Clone, Copy)]
enum RoutingPreference {
    Unconfigured,
    Managed(Option<i64>),
    Stale,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RoutingPolicy {
    OauthFirst,
    RelayFirst,
    Balanced,
}

impl RoutingPolicy {
    fn parse(raw: &str) -> Result<Self, String> {
        match raw.trim() {
            "oauthFirst" => Ok(Self::OauthFirst),
            "relayFirst" => Ok(Self::RelayFirst),
            "balanced" => Ok(Self::Balanced),
            _ => Err("路由策略必须是 oauthFirst、relayFirst 或 balanced".into()),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::OauthFirst => "oauthFirst",
            Self::RelayFirst => "relayFirst",
            Self::Balanced => "balanced",
        }
    }
}

#[derive(Debug, Clone)]
struct GatewayKeyBinding {
    id: i64,
    group_id: Option<i64>,
    status: String,
}

#[derive(Debug, Clone, Default)]
struct RoutingObservation {
    recent_request_count: u32,
    last_successful_account_id: Option<i64>,
    last_successful_account_name: Option<String>,
    last_successful_account_type: Option<String>,
    last_successful_at: Option<String>,
    distribution: Vec<Sub2ApiRoutingDistribution>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RoutingPriorityChange {
    account_id: i64,
    original_priority: i64,
    desired_priority: i64,
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
    let text =
        fs::read_to_string(&env_path).map_err(|e| format!("read {}: {e}", env_path.display()))?;
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

fn request_admin_token() -> Result<String, AdminLoginError> {
    let login_error = |message: String, retry_after: Duration| AdminLoginError {
        message,
        retry_after,
    };
    let (email, password) =
        read_admin_creds().map_err(|error| login_error(error, ADMIN_LOGIN_BACKOFF))?;
    let resp = HTTP
        .post(format!("{GATEWAY_BASE}/api/v1/auth/login"))
        .json(&json!({ "email": email, "password": password }))
        .send()
        .map_err(|error| {
            login_error(friendly_http_err("admin login", error), ADMIN_LOGIN_BACKOFF)
        })?;
    let status = resp.status();
    let retry_after = resp
        .headers()
        .get("retry-after")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(|seconds| Duration::from_secs(seconds.clamp(1, 300)));
    let text = resp.text().map_err(|error| {
        login_error(
            format!("read admin login response: {error}"),
            ADMIN_LOGIN_BACKOFF,
        )
    })?;
    let body = serde_json::from_str::<Value>(&text).ok();
    if !status.is_success() {
        let message = body
            .as_ref()
            .and_then(|body| body.get("message"))
            .and_then(Value::as_str)
            .unwrap_or(text.trim());
        let message = safe_reason(message, "request failed");
        let retry_after = retry_after.unwrap_or(if status.as_u16() == 429 {
            ADMIN_LOGIN_RATE_LIMIT_BACKOFF
        } else {
            ADMIN_LOGIN_BACKOFF
        });
        let retry_hint = format!("，请在 {} 秒后重试", retry_after.as_secs());
        return Err(login_error(
            format!("admin login HTTP {status}: {message}{retry_hint}"),
            retry_after,
        ));
    }
    let body = body.ok_or_else(|| {
        login_error(
            "parse admin login: response was not JSON".into(),
            ADMIN_LOGIN_BACKOFF,
        )
    })?;
    body.pointer("/data/access_token")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| {
            login_error(
                "admin login missing access_token".into(),
                ADMIN_LOGIN_BACKOFF,
            )
        })
}

pub(crate) fn admin_login() -> Result<String, String> {
    // Keep the lock across login so concurrent tray and card refreshes share a
    // single request instead of exhausting Sub2API's auth rate limiter.
    let mut auth = ADMIN_AUTH.lock();
    let now = Instant::now();
    if let Some(result) = auth.cached_result_at(now) {
        return result;
    }

    match request_admin_token() {
        Ok(value) => {
            *auth = AdminAuthState::Ready(CachedAdminToken {
                value: value.clone(),
                expires_at: Instant::now() + ADMIN_TOKEN_TTL,
            });
            Ok(value)
        }
        Err(error) => {
            let message = error.message;
            *auth = AdminAuthState::Backoff {
                retry_at: Instant::now() + error.retry_after,
                error: message.clone(),
            };
            Err(message)
        }
    }
}

pub(crate) fn invalidate_admin_token(value: &str) {
    let mut auth = ADMIN_AUTH.lock();
    if matches!(&*auth, AdminAuthState::Ready(token) if token.value == value) {
        *auth = AdminAuthState::Empty;
    }
}

fn admin_get(path: &str) -> Result<Value, String> {
    for attempt in 0..=1 {
        let token = admin_login()?;
        let resp = HTTP
            .get(format!("{GATEWAY_BASE}{path}"))
            .bearer_auth(&token)
            .header("User-Agent", BROWSER_UA)
            .send()
            .map_err(|e| friendly_http_err(&format!("admin {path}"), e))?;
        let status = resp.status();
        if status.as_u16() == 401 {
            invalidate_admin_token(&token);
            if attempt == 0 {
                continue;
            }
        }
        let body: Value = resp
            .json()
            .map_err(|e| format!("parse admin {path}: {e}"))?;
        if !status.is_success() {
            return Err(format!(
                "admin {path} HTTP {status}: {}",
                admin_error_reason(&body, "request failed")
            ));
        }
        return Ok(body);
    }
    unreachable!("admin GET retry loop always returns")
}

fn admin_post(path: &str, payload: Value) -> Result<Value, String> {
    for attempt in 0..=1 {
        let token = admin_login()?;
        let resp = HTTP
            .post(format!("{GATEWAY_BASE}{path}"))
            .bearer_auth(&token)
            .header("User-Agent", BROWSER_UA)
            .json(&payload)
            .send()
            .map_err(|e| friendly_http_err(&format!("admin POST {path}"), e))?;
        let status = resp.status();
        if status.as_u16() == 401 {
            invalidate_admin_token(&token);
            if attempt == 0 {
                continue;
            }
        }
        let body: Value = resp
            .json()
            .map_err(|e| format!("parse admin {path}: {e}"))?;
        if !status.is_success() {
            return Err(format!(
                "admin {path} HTTP {status}: {}",
                admin_error_reason(&body, "request failed")
            ));
        }
        return Ok(body);
    }
    unreachable!("admin POST retry loop always returns")
}

fn admin_put(path: &str, payload: Value) -> Result<Value, String> {
    for attempt in 0..=1 {
        let token = admin_login()?;
        let resp = HTTP
            .put(format!("{GATEWAY_BASE}{path}"))
            .bearer_auth(&token)
            .header("User-Agent", BROWSER_UA)
            .json(&payload)
            .send()
            .map_err(|e| friendly_http_err(&format!("admin PUT {path}"), e))?;
        let status = resp.status();
        if status.as_u16() == 401 {
            invalidate_admin_token(&token);
            if attempt == 0 {
                continue;
            }
        }
        let body: Value = resp
            .json()
            .map_err(|e| format!("parse admin {path}: {e}"))?;
        if !status.is_success() {
            return Err(format!(
                "admin PUT {path} HTTP {status}: {}",
                admin_error_reason(&body, "request failed")
            ));
        }
        return Ok(body);
    }
    unreachable!("admin PUT retry loop always returns")
}

fn admin_delete(path: &str) -> Result<(), String> {
    for attempt in 0..=1 {
        let token = admin_login()?;
        let resp = HTTP
            .delete(format!("{GATEWAY_BASE}{path}"))
            .bearer_auth(&token)
            .header("User-Agent", BROWSER_UA)
            .send()
            .map_err(|e| friendly_http_err(&format!("admin DELETE {path}"), e))?;
        let status = resp.status();
        if status.as_u16() == 401 {
            invalidate_admin_token(&token);
            if attempt == 0 {
                continue;
            }
        }
        if status.is_success() || status.as_u16() == 204 {
            return Ok(());
        }
        let body: Value = resp.json().unwrap_or(json!({}));
        return Err(format!(
            "admin DELETE {path} HTTP {status}: {}",
            admin_error_reason(&body, "request failed")
        ));
    }
    unreachable!("admin DELETE retry loop always returns")
}

fn admin_error_reason(body: &Value, fallback: &str) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .or_else(|| body.pointer("/error/message").and_then(Value::as_str))
        .map(|message| safe_reason(message, fallback))
        .unwrap_or_else(|| fallback.to_string())
}

fn gateway_healthy() -> bool {
    HTTP.get(format!("{GATEWAY_BASE}/health"))
        .timeout(Duration::from_secs(1))
        .send()
        .map(|response| response.status().is_success())
        .unwrap_or(false)
}

fn wait_for_gateway_health(timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if gateway_healthy() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(250));
    }
}

fn is_transient_gateway_connection_error(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    [
        "error sending request",
        "connection refused",
        "tcp connect",
        "connection reset",
        "timed out",
        "无法连接",
        "请求超时",
    ]
    .iter()
    .any(|needle| error.contains(needle))
}

fn clear_admin_login_backoff() {
    let mut auth = ADMIN_AUTH.lock();
    if matches!(&*auth, AdminAuthState::Backoff { .. }) {
        *auth = AdminAuthState::Empty;
    }
}

fn browser_admin_post(path: &str, payload: Value) -> Result<Value, String> {
    if !wait_for_gateway_health(Duration::from_secs(8)) {
        return Err(
            "Sub2API 本地网关尚未就绪（127.0.0.1:18080）。请确认容器和 SSH 隧道已启动后重试。"
                .into(),
        );
    }
    match admin_post(path, payload.clone()) {
        Ok(response) => Ok(response),
        Err(error) if is_transient_gateway_connection_error(&error) => {
            if !wait_for_gateway_health(Duration::from_secs(8)) {
                return Err(
                    "Sub2API 在登录过程中断开，等待恢复超时。请确认容器和 SSH 隧道后重试。".into(),
                );
            }
            clear_admin_login_backoff();
            admin_post(path, payload).map_err(|retry_error| {
                format!(
                    "Sub2API 已恢复，但管理员会话仍无法建立：{}",
                    safe_reason(&retry_error, "管理员登录失败")
                )
            })
        }
        Err(error) => Err(error),
    }
}

fn response_data(body: &Value) -> &Value {
    body.get("data").unwrap_or(body)
}

fn parse_timestamp(value: &Value) -> Option<DateTime<Utc>> {
    fn from_unix(raw: i64) -> Option<DateTime<Utc>> {
        if raw <= 0 {
            return None;
        }
        let (seconds, nanos) = if raw > 10_000_000_000 {
            (raw / 1_000, ((raw % 1_000) * 1_000_000) as u32)
        } else {
            (raw, 0)
        };
        DateTime::from_timestamp(seconds, nanos)
    }

    if let Some(raw) = value.as_i64() {
        return from_unix(raw);
    }
    if let Some(raw) = value.as_u64().and_then(|raw| i64::try_from(raw).ok()) {
        return from_unix(raw);
    }
    let raw = value.as_str()?.trim();
    if raw.is_empty() {
        return None;
    }
    if let Ok(unix) = raw.parse::<i64>() {
        return from_unix(unix);
    }
    DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|timestamp| timestamp.with_timezone(&Utc))
}

pub(crate) fn safe_reason(raw: &str, fallback: &str) -> String {
    let normalized = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let lowered = normalized.to_ascii_lowercase();
    if [
        "access_token",
        "refresh_token",
        "api_key",
        "authorization:",
        "bearer ",
    ]
    .iter()
    .any(|needle| lowered.contains(needle))
    {
        return fallback.to_string();
    }
    let value = if normalized.is_empty() {
        fallback
    } else {
        &normalized
    };
    value.chars().take(240).collect()
}

fn latest_model_rate_limit(account: &Value, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    account
        .pointer("/extra/model_rate_limits")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|limits| limits.values())
        .filter_map(|entry| {
            entry
                .get("rate_limit_reset_at")
                .or_else(|| entry.as_str().map(|_| entry))
                .and_then(parse_timestamp)
        })
        .filter(|reset_at| *reset_at > now)
        .max()
}

fn is_permanent_account_error(raw_status: &str, error_message: &str) -> bool {
    let text = format!("{raw_status} {error_message}").to_ascii_lowercase();
    [
        "banned",
        "revoked",
        "invalid_grant",
        "invalid refresh token",
        "credential expired",
        "account disabled",
        "unauthorized",
    ]
    .iter()
    .any(|needle| text.contains(needle))
}

fn temporary_unavailable_reason(account: &Value) -> String {
    let raw = account
        .get("temp_unschedulable_reason")
        .and_then(Value::as_str)
        .unwrap_or("");
    if let Ok(payload) = serde_json::from_str::<Value>(raw) {
        if payload.get("source").and_then(Value::as_str) == Some("account_scheduling_threshold") {
            let threshold = payload.get("threshold").and_then(Value::as_f64);
            let window = payload.get("window").and_then(Value::as_str);
            return match (threshold, window) {
                (Some(threshold), Some(window)) => {
                    format!("{window} 额度达到 {:.0}% 自动摘除阈值", threshold)
                }
                (Some(threshold), None) => {
                    format!("额度达到 {:.0}% 自动摘除阈值", threshold)
                }
                _ => "额度达到自动摘除阈值".into(),
            };
        }
    }
    safe_reason(raw, "账号暂时不可调度")
}

fn classify_account_availability(account: &Value, now: DateTime<Utc>) -> AccountAvailability {
    let raw_status = account.get("status").and_then(Value::as_str).unwrap_or("");
    let error_message = account
        .get("error_message")
        .and_then(Value::as_str)
        .unwrap_or("");
    let status = normalize_status(raw_status, error_message);
    let unavailable = |availability: &str,
                       reason: String,
                       recoverable: bool,
                       until: Option<DateTime<Utc>>| AccountAvailability {
        available: false,
        availability: availability.to_string(),
        reason,
        recoverable,
        unavailable_until: until.map(|timestamp| timestamp.to_rfc3339()),
    };

    if account
        .get("expires_at")
        .and_then(parse_timestamp)
        .is_some_and(|expires_at| expires_at <= now)
    {
        return unavailable("expired", "账号凭据已过期".into(), false, None);
    }

    if status == "error" {
        let permanent = is_permanent_account_error(raw_status, error_message);
        return unavailable(
            "error",
            safe_reason(error_message, "账号状态异常"),
            !permanent,
            None,
        );
    }
    if status != "ready" {
        return unavailable(
            "inactive",
            safe_reason(raw_status, "账号未启用"),
            false,
            None,
        );
    }

    if let Some(reset_at) = account
        .get("rate_limit_reset_at")
        .and_then(parse_timestamp)
        .filter(|reset_at| *reset_at > now)
    {
        return unavailable(
            "rate_limited",
            "账号已触发全局限流".into(),
            true,
            Some(reset_at),
        );
    }

    if let Some(overload_until) = account
        .get("overload_until")
        .and_then(parse_timestamp)
        .filter(|overload_until| *overload_until > now)
    {
        return unavailable(
            "overloaded",
            "上游暂时过载".into(),
            true,
            Some(overload_until),
        );
    }

    if let Some(temporary_until) = account
        .get("temp_unschedulable_until")
        .and_then(parse_timestamp)
        .filter(|temporary_until| *temporary_until > now)
    {
        return unavailable(
            "temporary",
            temporary_unavailable_reason(account),
            true,
            Some(temporary_until),
        );
    }

    let schedulable = account
        .get("schedulable")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    if !schedulable {
        return unavailable("paused", "账号已暂停调度".into(), false, None);
    }

    if let Some(reset_at) = latest_model_rate_limit(account, now) {
        return AccountAvailability {
            available: true,
            availability: "model_rate_limited".into(),
            reason: "部分模型已停车，其他模型仍可调度".into(),
            recoverable: true,
            unavailable_until: Some(reset_at.to_rfc3339()),
        };
    }

    AccountAvailability {
        available: true,
        availability: "ready".into(),
        reason: "账号可调度".into(),
        recoverable: false,
        unavailable_until: None,
    }
}

#[cfg(test)]
fn preferred_group_name(account_id: i64) -> String {
    format!("{PREFERRED_OAUTH_GROUP_PREFIX}{account_id}")
}

#[cfg(test)]
fn routing_group_name(preferred_account_id: Option<i64>, policy: RoutingPolicy) -> String {
    match preferred_account_id {
        Some(account_id) => format!(
            "{PREFERRED_OAUTH_GROUP_PREFIX}{account_id}:{}",
            policy.as_str()
        ),
        None => format!("{AUTOMATIC_GROUP_PREFIX}{}", policy.as_str()),
    }
}

fn parse_preferred_account_id(group_name: &str) -> Option<i64> {
    let account_id = group_name
        .strip_prefix(PREFERRED_OAUTH_GROUP_PREFIX)?
        .split(':')
        .next()?
        .parse::<i64>()
        .ok()?;
    (account_id > 0).then_some(account_id)
}

fn validate_auto_pause_threshold(percent: u8) -> Result<(), String> {
    if !(1..=100).contains(&percent) {
        return Err("自动暂停阈值必须在 1 到 100 之间（100 表示关闭）".into());
    }
    Ok(())
}

fn scheduling_thresholds_from_settings(settings: &Value) -> Map<String, Value> {
    let source = response_data(settings)
        .get("account_scheduling_thresholds")
        .and_then(Value::as_object);
    let mut thresholds = Map::new();
    for platform in ["openai", "anthropic", "grok"] {
        let percent = source
            .and_then(|values| values.get(platform))
            .and_then(Value::as_u64)
            .filter(|percent| (1..=100).contains(percent))
            .unwrap_or(100);
        thresholds.insert(platform.to_string(), Value::from(percent));
    }
    thresholds
}

fn scheduling_thresholds_payload(settings: &Value, openai_percent: u8) -> Value {
    let mut thresholds = scheduling_thresholds_from_settings(settings);
    thresholds.insert("openai".into(), Value::from(openai_percent));
    json!({ "account_scheduling_thresholds": thresholds })
}

fn openai_threshold_from_settings(settings: &Value) -> u8 {
    scheduling_thresholds_from_settings(settings)
        .get("openai")
        .and_then(Value::as_u64)
        .and_then(|value| u8::try_from(value).ok())
        .unwrap_or(100)
}

fn auto_pause_sentinel_path() -> PathBuf {
    sub2api_dir().join("state").join(AUTO_PAUSE_SENTINEL)
}

fn write_auto_pause_sentinel() -> Result<(), String> {
    let path = auto_pause_sentinel_path();
    let parent = path
        .parent()
        .ok_or_else(|| "自动暂停状态目录无效".to_string())?;
    fs::create_dir_all(parent)
        .map_err(|e| format!("创建自动暂停状态目录 {}: {e}", parent.display()))?;
    let temp_path = parent.join(format!(".{AUTO_PAUSE_SENTINEL}.{}.tmp", Uuid::new_v4()));
    fs::write(&temp_path, b"initialized\n")
        .map_err(|e| format!("写入自动暂停状态 {}: {e}", temp_path.display()))?;
    if let Err(error) = fs::rename(&temp_path, &path) {
        let _ = fs::remove_file(&temp_path);
        return Err(format!("保存自动暂停状态 {}: {error}", path.display()));
    }
    Ok(())
}

fn load_or_initialize_auto_pause_threshold() -> Result<u8, String> {
    let _guard = AUTO_PAUSE_INITIALIZATION.lock();
    let settings = admin_get("/api/v1/admin/settings")?;
    if cfg!(test) {
        // Existing live tests read the local Sub2API instance. Test binaries
        // must never provision settings or create the local initialization file.
        return Ok(openai_threshold_from_settings(&settings));
    }
    let current_percent = openai_threshold_from_settings(&settings);
    if auto_pause_sentinel_path().is_file() {
        return Ok(current_percent);
    }
    if current_percent != 100 {
        // Respect an administrator's existing non-default threshold and mark
        // the one-time Hub default as already handled.
        write_auto_pause_sentinel()?;
        return Ok(current_percent);
    }

    // Persist the initialization marker without rewriting the upstream default
    // of 100. This both avoids a SETTING_NOT_FOUND-driven surprise mutation and
    // prevents stale local quota snapshots from parking an officially usable
    // OAuth account for as long as its seven-day reset window.
    write_auto_pause_sentinel()?;
    Ok(AUTO_PAUSE_DEFAULT_PERCENT)
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
    // A window with no time left is not a real active window — Sub2API emits
    // phantom nodes (utilization 0, resets_at in the past) when upstream does
    // not report that window at all. Treat them as "no data".
    if reset == 0 {
        return None;
    }
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

fn avg_window(windows: &[QuotaWindow]) -> Option<QuotaWindow> {
    if windows.is_empty() {
        return None;
    }
    let n = windows.len() as f64;
    let remaining = windows.iter().map(|w| w.remaining_percent).sum::<f64>() / n;
    let reset = windows
        .iter()
        .map(|w| w.reset_after_seconds)
        .min()
        .unwrap_or(0);
    Some(QuotaWindow {
        remaining_percent: remaining,
        reset_after_seconds: reset,
    })
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

fn gui_child_path() -> String {
    let mut path = std::env::var("PATH").unwrap_or_default();
    for extra in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin"] {
        if !path.split(':').any(|segment| segment == extra) {
            if !path.is_empty() {
                path.push(':');
            }
            path.push_str(extra);
        }
    }
    path
}

fn clear_group_sticky_sessions(group_id: i64) -> Result<u32, String> {
    if group_id <= 0 {
        return Err("无效的 Sub2API group_id".into());
    }
    let prefix = format!("sticky_session:{group_id}:");
    let pattern = format!("{prefix}*");
    let scan = Command::new("docker")
        .args([
            "exec",
            "sub2api-json-proxy-redis",
            "redis-cli",
            "--raw",
            "--scan",
            "--pattern",
            &pattern,
        ])
        .env("PATH", gui_child_path())
        .output()
        .map_err(|error| format!("扫描 Sub2API 粘性会话失败：{error}"))?;
    if !scan.status.success() {
        return Err("扫描 Sub2API 粘性会话失败：Redis 不可用".into());
    }
    let output = String::from_utf8(scan.stdout)
        .map_err(|_| "扫描 Sub2API 粘性会话失败：Redis 返回无效文本".to_string())?;
    let keys = output
        .lines()
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .collect::<Vec<_>>();
    if keys.is_empty() {
        return Ok(0);
    }
    if keys.len() > 10_000
        || keys
            .iter()
            .any(|key| !key.starts_with(&prefix) || key.len() > 512)
    {
        return Err("拒绝清理范围异常的 Sub2API 粘性会话".into());
    }

    // Feed RESP commands over stdin so individual session hashes never appear
    // in a child-process argv or Hub log.
    let mut commands = Vec::new();
    for key in &keys {
        commands.extend_from_slice(format!("*2\r\n$3\r\nDEL\r\n${}\r\n", key.len()).as_bytes());
        commands.extend_from_slice(key.as_bytes());
        commands.extend_from_slice(b"\r\n");
    }
    let mut child = Command::new("docker")
        .args([
            "exec",
            "-i",
            "sub2api-json-proxy-redis",
            "redis-cli",
            "--pipe",
        ])
        .env("PATH", gui_child_path())
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("清理 Sub2API 粘性会话失败：{error}"))?;
    child
        .stdin
        .as_mut()
        .ok_or_else(|| "无法写入 Redis 清理命令".to_string())?
        .write_all(&commands)
        .map_err(|error| format!("写入 Redis 清理命令失败：{error}"))?;
    let result = child
        .wait_with_output()
        .map_err(|error| format!("等待 Redis 清理完成失败：{error}"))?;
    if !result.status.success() {
        return Err("清理 Sub2API 粘性会话失败：Redis pipe 执行失败".into());
    }
    Ok(keys.len() as u32)
}

fn set_sub2api_app_container_state(action: &str) -> Result<(), String> {
    if !matches!(action, "stop" | "start") {
        return Err("不支持的 Sub2API 容器操作".into());
    }
    let output = Command::new("docker")
        .args([action, "sub2api-json-proxy-app"])
        .env("PATH", gui_child_path())
        .output()
        .map_err(|error| format!("{action} Sub2API app 容器失败：{error}"))?;
    if !output.status.success() {
        return Err(format!(
            "{action} Sub2API app 容器失败，请在路由医生中复核服务状态"
        ));
    }
    Ok(())
}

fn activate_routing_change(group_id: i64) -> Result<(), String> {
    // Stop first so an in-flight HTTP/WS request cannot recreate a sticky key
    // between eviction and restart. Always attempt to start again even if the
    // scoped Redis cleanup fails.
    set_sub2api_app_container_state("stop")?;
    let clear_result = clear_group_sticky_sessions(group_id);
    let start_result = set_sub2api_app_container_state("start");
    if let Err(error) = start_result {
        return Err(error);
    }
    if !wait_for_gateway_health(Duration::from_secs(15)) {
        return Err("Sub2API app 容器已启动，但 15 秒内未恢复健康".into());
    }
    clear_result.map(|_| ())
}

fn sub2api_env_value(key: &str, fallback: &str) -> String {
    fs::read_to_string(sub2api_dir().join(".env"))
        .ok()
        .and_then(|text| {
            text.lines().find_map(|line| {
                let line = line.trim();
                let (name, value) = line.split_once('=')?;
                (name.trim() == key).then(|| {
                    value
                        .trim()
                        .trim_matches('"')
                        .trim_matches('\'')
                        .to_string()
                })
            })
        })
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

fn read_official_quota_credentials(account_id: i64) -> Result<(String, String), String> {
    if account_id <= 0 {
        return Err("无效的 account_id".into());
    }
    let database_user = sub2api_env_value("POSTGRES_USER", "sub2api");
    let database_name = sub2api_env_value("POSTGRES_DB", "sub2api");
    let query = format!(
        "SELECT COALESCE(credentials->>'access_token',''), COALESCE(credentials->>'chatgpt_account_id','') FROM accounts WHERE id={account_id} AND platform='openai' AND type='oauth' AND deleted_at IS NULL"
    );
    let output = Command::new("docker")
        .args([
            "exec",
            "sub2api-json-proxy-postgres",
            "psql",
            "-U",
            &database_user,
            "-d",
            &database_name,
            "-At",
            "-F",
            "\t",
            "-c",
            &query,
        ])
        .env("PATH", gui_child_path())
        .output()
        .map_err(|error| format!("读取官方额度凭据失败：无法执行本机 Docker（{error}）"))?;
    if !output.status.success() {
        return Err("读取官方额度凭据失败：Sub2API Postgres 不可用".into());
    }
    let row = String::from_utf8(output.stdout)
        .map_err(|_| "读取官方额度凭据失败：数据库返回了无效文本".to_string())?;
    let (access_token, chatgpt_account_id) = row
        .trim_end()
        .split_once('\t')
        .ok_or_else(|| "该 OAuth 账号没有可用于官方探测的凭据".to_string())?;
    if access_token.is_empty() || chatgpt_account_id.is_empty() {
        return Err("该 OAuth 账号缺少 access_token 或 chatgpt_account_id".into());
    }
    Ok((access_token.to_string(), chatgpt_account_id.to_string()))
}

fn official_window(node: &Value) -> Option<(u64, OfficialQuotaWindow)> {
    let used_percent = node.get("used_percent")?.as_f64()?.clamp(0.0, 100.0);
    let window_seconds = node
        .get("limit_window_seconds")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let reset_after_seconds = node
        .get("reset_after_seconds")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let reset_at = node
        .get("reset_at")
        .and_then(parse_timestamp)
        .map(|timestamp| timestamp.to_rfc3339());
    let limit_reached = node
        .get("limit_reached")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || used_percent >= 100.0;
    Some((
        window_seconds,
        OfficialQuotaWindow {
            used_percent,
            limit_reached,
            reset_after_seconds,
            reset_at,
        },
    ))
}

fn parse_official_quota(account_id: i64, body: &Value) -> Result<OfficialQuotaProbe, String> {
    let rate_limit = body
        .get("rate_limit")
        .ok_or_else(|| "官方额度响应缺少 rate_limit".to_string())?;
    let root_limit_reached = rate_limit
        .get("limit_reached")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let allowed = rate_limit
        .get("allowed")
        .and_then(Value::as_bool)
        .unwrap_or(!root_limit_reached);
    let mut five_hour = None;
    let mut seven_day = None;
    for name in ["primary_window", "secondary_window"] {
        let Some((window_seconds, window)) = rate_limit.get(name).and_then(official_window) else {
            continue;
        };
        // Current OpenAI payloads use 18,000 seconds for the 5h window and
        // 604,800 for the 7d window. Classify by duration instead of assuming
        // primary/secondary ordering, which has changed across deployments.
        if window_seconds > 86_400 {
            seven_day = Some(window);
        } else {
            five_hour = Some(window);
        }
    }
    Ok(OfficialQuotaProbe {
        account_id,
        plan_type: body
            .get("plan_type")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string(),
        allowed,
        limit_reached: root_limit_reached,
        five_hour,
        seven_day,
        fetched_at: now_iso(),
    })
}

/// Probe OpenAI's authoritative ChatGPT/Codex quota. The OAuth token is read
/// from local Postgres into process memory and is never returned or logged.
#[tauri::command]
pub fn probe_sub2api_official_quota(account_id: i64) -> Result<OfficialQuotaProbe, String> {
    validate_openai_oauth_account(account_id)?;
    let (access_token, chatgpt_account_id) = read_official_quota_credentials(account_id)?;
    let response = HTTP
        .get("https://chatgpt.com/backend-api/wham/usage")
        .bearer_auth(access_token)
        .header("chatgpt-account-id", chatgpt_account_id)
        .header("Accept", "application/json")
        .send()
        .map_err(|error| friendly_http_err("官方额度探测", error))?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("官方额度探测失败（HTTP {status}）"));
    }
    let body = response
        .json::<Value>()
        .map_err(|error| format!("解析官方额度响应失败：{error}"))?;
    parse_official_quota(account_id, &body)
}

fn list_oauth_accounts() -> Result<Vec<Value>, String> {
    let mut out = Vec::new();
    let mut page = 1u32;
    loop {
        let body = admin_get(&format!(
            "/api/v1/admin/accounts?page={page}&page_size=50&type=oauth&platform=openai"
        ))?;
        let items = body
            .pointer("/data/items")
            .and_then(|v| v.as_array())
            .cloned()
            .or_else(|| body.get("data").and_then(|d| d.as_array()).cloned())
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
    // Some builds ignore one or both filters, so enforce the pool boundary
    // client-side as well.
    out.retain(|account| {
        account.get("type").and_then(Value::as_str) == Some("oauth")
            && account.get("platform").and_then(Value::as_str) == Some("openai")
    });
    if out.is_empty() {
        let body = admin_get("/api/v1/admin/accounts?page=1&page_size=100")?;
        let items = body
            .pointer("/data/items")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        out = items
            .into_iter()
            .filter(|account| {
                account.get("type").and_then(Value::as_str) == Some("oauth")
                    && account.get("platform").and_then(Value::as_str) == Some("openai")
            })
            .collect();
    }
    Ok(out)
}

fn list_openai_accounts() -> Result<Vec<Value>, String> {
    let mut out = Vec::new();
    let mut page = 1u32;
    loop {
        let body = admin_get(&format!(
            "/api/v1/admin/accounts?page={page}&page_size=100&platform=openai"
        ))?;
        let items = body
            .pointer("/data/items")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let count = items.len();
        out.extend(
            items.into_iter().filter(|account| {
                account.get("platform").and_then(Value::as_str) == Some("openai")
            }),
        );
        if count < 100 || page >= 20 {
            break;
        }
        page += 1;
    }
    Ok(out)
}

fn routing_policy_path() -> PathBuf {
    sub2api_dir().join("state").join(ROUTING_POLICY_FILE)
}

fn write_routing_policy(policy: RoutingPolicy) -> Result<(), String> {
    let path = routing_policy_path();
    let parent = path
        .parent()
        .ok_or_else(|| "路由策略状态目录无效".to_string())?;
    fs::create_dir_all(parent)
        .map_err(|e| format!("创建路由策略状态目录 {}: {e}", parent.display()))?;
    let temp_path = parent.join(format!(".{ROUTING_POLICY_FILE}.{}.tmp", Uuid::new_v4()));
    fs::write(&temp_path, format!("{}\n", policy.as_str()))
        .map_err(|e| format!("写入路由策略状态 {}: {e}", temp_path.display()))?;
    if let Err(error) = fs::rename(&temp_path, &path) {
        let _ = fs::remove_file(&temp_path);
        return Err(format!("保存路由策略状态 {}: {error}", path.display()));
    }
    Ok(())
}

fn read_routing_policy() -> RoutingPolicy {
    fs::read_to_string(routing_policy_path())
        .ok()
        .and_then(|raw| RoutingPolicy::parse(raw.trim()).ok())
        .unwrap_or(RoutingPolicy::OauthFirst)
}

fn stored_routing_policy() -> Option<RoutingPolicy> {
    let path = routing_policy_path();
    if !path.is_file() {
        return None;
    }
    fs::read_to_string(path)
        .ok()
        .and_then(|raw| RoutingPolicy::parse(raw.trim()).ok())
}

fn routing_policy_modified_at() -> Option<DateTime<Utc>> {
    fs::metadata(routing_policy_path())
        .ok()?
        .modified()
        .ok()
        .map(DateTime::<Utc>::from)
}

fn recent_routing_cutoff(
    now: DateTime<Utc>,
    policy_modified_at: Option<DateTime<Utc>>,
) -> DateTime<Utc> {
    let rolling_cutoff = now - chrono::Duration::minutes(RECENT_ROUTE_WINDOW_MINUTES);
    policy_modified_at
        .filter(|modified_at| *modified_at > rolling_cutoff)
        .unwrap_or(rolling_cutoff)
}

fn routing_preferred_account_path() -> PathBuf {
    sub2api_dir()
        .join("state")
        .join(ROUTING_PREFERRED_ACCOUNT_FILE)
}

fn write_routing_preference(preferred_account_id: Option<i64>) -> Result<(), String> {
    if preferred_account_id.is_some_and(|account_id| account_id <= 0) {
        return Err("无效的首选账号 ID".into());
    }
    let path = routing_preferred_account_path();
    let parent = path
        .parent()
        .ok_or_else(|| "路由首选状态目录无效".to_string())?;
    fs::create_dir_all(parent)
        .map_err(|e| format!("创建路由首选状态目录 {}: {e}", parent.display()))?;
    let temp_path = parent.join(format!(
        ".{ROUTING_PREFERRED_ACCOUNT_FILE}.{}.tmp",
        Uuid::new_v4()
    ));
    let value = preferred_account_id
        .map(|account_id| account_id.to_string())
        .unwrap_or_else(|| "automatic".into());
    fs::write(&temp_path, format!("{value}\n"))
        .map_err(|e| format!("写入路由首选状态 {}: {e}", temp_path.display()))?;
    if let Err(error) = fs::rename(&temp_path, &path) {
        let _ = fs::remove_file(&temp_path);
        return Err(format!("保存路由首选状态 {}: {error}", path.display()));
    }
    Ok(())
}

fn read_stored_routing_preference() -> Option<RoutingPreference> {
    let raw = fs::read_to_string(routing_preferred_account_path()).ok()?;
    let value = raw.trim();
    if value == "automatic" {
        return Some(RoutingPreference::Managed(None));
    }
    let account_id = value
        .parse::<i64>()
        .ok()
        .filter(|account_id| *account_id > 0)?;
    Some(RoutingPreference::Managed(Some(account_id)))
}

fn desired_account_priority(policy: RoutingPolicy, account_type: &str, preferred: bool) -> i64 {
    match policy {
        RoutingPolicy::OauthFirst => {
            if preferred {
                0
            } else if account_type == "oauth" {
                10
            } else {
                100
            }
        }
        RoutingPolicy::RelayFirst => {
            if account_type != "oauth" {
                0
            } else if preferred {
                50
            } else {
                100
            }
        }
        RoutingPolicy::Balanced => {
            if preferred {
                0
            } else {
                50
            }
        }
    }
}

fn group_ids_from_account(account: &Value) -> Vec<i64> {
    let mut ids = account
        .get("group_ids")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_i64)
        .filter(|group_id| *group_id > 0)
        .collect::<Vec<_>>();
    ids.extend(
        account
            .get("account_groups")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|membership| membership.get("group_id").and_then(Value::as_i64))
            .filter(|group_id| *group_id > 0),
    );
    ids.sort_unstable();
    ids.dedup();
    ids
}

fn resolve_openai_accounts_for_group(
    accounts: &[Value],
    group_id: i64,
) -> Result<Vec<Value>, String> {
    if group_id <= 0 {
        return Err("当前网关 API key 未绑定有效分组".into());
    }
    let mut members = Vec::new();
    for account in accounts {
        let Some(account_id) = account
            .get("id")
            .and_then(Value::as_i64)
            .filter(|account_id| *account_id > 0)
        else {
            continue;
        };
        let membership_ids = group_ids_from_account(account);
        if !membership_ids.is_empty() {
            if membership_ids.contains(&group_id) {
                if account.get("priority").and_then(Value::as_i64).is_some() {
                    members.push(account.clone());
                } else {
                    let detail = admin_get(&format!("/api/v1/admin/accounts/{account_id}"))?;
                    let detail = response_data(&detail).clone();
                    if group_ids_from_account(&detail).contains(&group_id) {
                        members.push(detail);
                    }
                }
            }
            continue;
        }

        // Some Sub2API list responses omit membership metadata. Fetching the
        // detail keeps another group's account from contaminating production
        // availability, policy, or traffic assertions.
        let detail = admin_get(&format!("/api/v1/admin/accounts/{account_id}"))?;
        let detail = response_data(&detail).clone();
        if group_ids_from_account(&detail).contains(&group_id) {
            members.push(detail);
        }
    }
    Ok(members)
}

fn plan_routing_priority_changes(
    members: &[Value],
    group_id: i64,
    preferred_account_id: Option<i64>,
    policy: RoutingPolicy,
    now: DateTime<Utc>,
) -> Result<Vec<RoutingPriorityChange>, String> {
    if group_id <= 0 || members.is_empty() {
        return Err(format!(
            "当前生产分组 #{group_id} 没有可用于路由的 OpenAI 账号"
        ));
    }

    let relay_ready = members.iter().any(|account| {
        account
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(is_relay_account_type)
            && classify_account_availability(account, now).availability == "ready"
    });
    if !relay_ready {
        return Err(format!(
            "拒绝修改路由：生产分组 #{group_id} 没有 active、schedulable 且无冷却的 apikey 中转兜底；请先用路由医生恢复 AIHub/AnyRouter"
        ));
    }

    if preferred_account_id.is_some_and(|preferred_id| {
        !members.iter().any(|account| {
            account.get("id").and_then(Value::as_i64) == Some(preferred_id)
                && account.get("type").and_then(Value::as_str) == Some("oauth")
        })
    }) {
        return Err(format!(
            "首选 OAuth 账号不属于当前生产分组 #{group_id}，未修改路由"
        ));
    }

    let mut seen = std::collections::HashSet::new();
    let mut changes = Vec::with_capacity(members.len());
    for account in members {
        let account_id = account
            .get("id")
            .and_then(Value::as_i64)
            .filter(|account_id| *account_id > 0)
            .ok_or_else(|| "生产分组成员缺少有效账号 ID，未修改路由".to_string())?;
        if !seen.insert(account_id) {
            return Err(format!("生产分组存在重复账号 #{account_id}，未修改路由"));
        }
        let account_type = account.get("type").and_then(Value::as_str).unwrap_or("");
        if account_type != "oauth" && !is_relay_account_type(account_type) {
            return Err(format!(
                "生产分组账号 #{account_id} 类型 {account_type:?} 不受 Hub 路由策略支持，未修改路由"
            ));
        }
        let original_priority = account
            .get("priority")
            .and_then(Value::as_i64)
            .ok_or_else(|| format!("生产分组账号 #{account_id} 缺少 priority，未修改路由"))?;
        changes.push(RoutingPriorityChange {
            account_id,
            original_priority,
            desired_priority: desired_account_priority(
                policy,
                account_type,
                preferred_account_id == Some(account_id),
            ),
        });
    }
    Ok(changes)
}

fn validate_production_group_relay_policy(group_id: i64) -> Result<(), String> {
    let groups = list_openai_groups(true)?;
    let group = groups
        .iter()
        .find(|group| group.get("id").and_then(Value::as_i64) == Some(group_id))
        .ok_or_else(|| format!("当前生产分组 #{group_id} 不存在，未修改路由"))?;
    let status = group.get("status").and_then(Value::as_str).unwrap_or("");
    if !matches!(status, "active" | "ready") {
        return Err(format!(
            "当前生产分组 #{group_id} 未启用（status={status}），未修改路由"
        ));
    }
    if group
        .get("require_oauth_only")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Err(format!(
            "拒绝修改路由：生产分组 #{group_id} 的 require_oauth_only=true 会过滤 apikey 中转兜底；请先用路由医生修复"
        ));
    }
    Ok(())
}

fn account_priority(account_id: i64) -> Result<i64, String> {
    let detail = admin_get(&format!("/api/v1/admin/accounts/{account_id}"))?;
    response_data(&detail)
        .get("priority")
        .and_then(Value::as_i64)
        .ok_or_else(|| format!("账号 #{account_id} 的 priority 无法核验"))
}

fn set_and_verify_account_priority(account_id: i64, expected: i64) -> Result<(), String> {
    let write_result = admin_put(
        &format!("/api/v1/admin/accounts/{account_id}"),
        json!({ "priority": expected }),
    );
    // A transport error is an indeterminate write: the server may have
    // committed before the response was lost. Always read back the actual
    // value instead of assuming failure or success from the response alone.
    match account_priority(account_id) {
        Ok(actual) if actual == expected => Ok(()),
        Ok(actual) => Err(format!(
            "账号 #{account_id} priority 核验失败：期望 {expected}，实际 {actual}{}",
            write_result
                .err()
                .map(|error| format!("；写入响应：{}", safe_reason(&error, "请求状态不确定")))
                .unwrap_or_default()
        )),
        Err(verify_error) => Err(match write_result {
            Ok(_) => format!(
                "账号 #{account_id} priority 写入后无法核验：{}",
                safe_reason(&verify_error, "读取失败")
            ),
            Err(write_error) => format!(
                "账号 #{account_id} priority 写入状态不确定：{}；且无法核验：{}",
                safe_reason(&write_error, "请求状态不确定"),
                safe_reason(&verify_error, "读取失败")
            ),
        }),
    }
}

fn rollback_routing_priorities(applied: &[RoutingPriorityChange]) -> Vec<i64> {
    let mut failed = Vec::new();
    for change in applied.iter().rev() {
        if set_and_verify_account_priority(change.account_id, change.original_priority).is_err() {
            failed.push(change.account_id);
        }
    }
    failed
}

fn routing_policy_matches_accounts(
    accounts: &[Value],
    group_id: i64,
    preferred_account_id: Option<i64>,
    policy: RoutingPolicy,
) -> bool {
    let members = accounts
        .iter()
        .filter(|account| group_ids_from_account(account).contains(&group_id))
        .collect::<Vec<_>>();
    if members.is_empty() {
        return false;
    }
    if preferred_account_id.is_some_and(|preferred_id| {
        !members.iter().any(|account| {
            account.get("id").and_then(Value::as_i64) == Some(preferred_id)
                && account.get("type").and_then(Value::as_str) == Some("oauth")
        })
    }) {
        return false;
    }
    members.iter().all(|account| {
        let Some(account_id) = account
            .get("id")
            .and_then(Value::as_i64)
            .filter(|account_id| *account_id > 0)
        else {
            return false;
        };
        let Some(account_type) = account.get("type").and_then(Value::as_str) else {
            return false;
        };
        let Some(actual_priority) = account.get("priority").and_then(Value::as_i64) else {
            return false;
        };
        actual_priority
            == desired_account_priority(
                policy,
                account_type,
                preferred_account_id == Some(account_id),
            )
    })
}

fn routing_availability_for_group(
    accounts: &[Value],
    group_id: i64,
    preferred_account_id: Option<i64>,
    now: DateTime<Utc>,
) -> (bool, u32, u32) {
    let available_members = accounts.iter().filter(|account| {
        group_ids_from_account(account).contains(&group_id)
            && classify_account_availability(account, now).available
    });
    available_members.fold(
        (false, 0_u32, 0_u32),
        |(fallback, relay_count, oauth_count), account| {
            let account_id = account.get("id").and_then(Value::as_i64);
            let account_type = account.get("type").and_then(Value::as_str).unwrap_or("");
            (
                fallback || account_id != preferred_account_id,
                relay_count + u32::from(is_relay_account_type(account_type)),
                oauth_count + u32::from(account_type.eq_ignore_ascii_case("oauth")),
            )
        },
    )
}

fn apply_routing_priorities(
    accounts: &[Value],
    group_id: i64,
    preferred_account_id: Option<i64>,
    policy: RoutingPolicy,
) -> Result<Vec<RoutingPriorityChange>, String> {
    validate_production_group_relay_policy(group_id)?;
    let members = resolve_openai_accounts_for_group(accounts, group_id)?;
    let changes = plan_routing_priority_changes(
        &members,
        group_id,
        preferred_account_id,
        policy,
        Utc::now(),
    )?;
    let mut applied = Vec::new();
    for change in &changes {
        if change.original_priority == change.desired_priority {
            continue;
        }
        // v0.1.173's OpenAI scheduler treats a lower accounts.priority value as
        // higher priority. Do not send group_ids here: UpdateAccount would
        // delete/recreate every account_groups row and replace its priority
        // with the group-array position instead of `desired`.
        // Add the current write to the rollback set before issuing it because
        // a lost HTTP response cannot tell us whether the server committed.
        applied.push(change.clone());
        if let Err(error) =
            set_and_verify_account_priority(change.account_id, change.desired_priority)
        {
            let rollback_failed = rollback_routing_priorities(&applied);
            let rollback = if rollback_failed.is_empty() {
                "已回滚此前更新".to_string()
            } else {
                format!(
                    "回滚账号 {} 失败，策略状态保持未配置，请立即运行路由医生",
                    rollback_failed
                        .iter()
                        .map(|id| format!("#{id}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            };
            return Err(format!(
                "更新生产组账号 #{} 优先级失败：{}；{rollback}",
                change.account_id,
                safe_reason(&error, "Sub2API 拒绝更新")
            ));
        }
    }
    Ok(applied)
}

fn apply_and_activate_routing_priorities(
    accounts: &[Value],
    group_id: i64,
    preferred_account_id: Option<i64>,
    policy: RoutingPolicy,
) -> Result<(), String> {
    let applied = apply_routing_priorities(accounts, group_id, preferred_account_id, policy)?;
    if let Err(error) = activate_routing_change(group_id) {
        let rollback_failed = rollback_routing_priorities(&applied);
        let rollback = if rollback_failed.is_empty() {
            "优先级已回滚，Hub 未写入已配置状态".to_string()
        } else {
            format!(
                "账号 {} 回滚失败，Hub 未写入已配置状态；服务恢复后请立即运行路由医生",
                rollback_failed
                    .iter()
                    .map(|id| format!("#{id}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        return Err(format!(
            "路由优先级已更新，但激活失败：{}；{rollback}",
            safe_reason(&error, "Sub2API 容器未恢复健康")
        ));
    }
    Ok(())
}

fn exact_gateway_key_binding(
    items: &[Value],
    gateway_key: &str,
) -> Result<GatewayKeyBinding, String> {
    let binding = items
        .iter()
        .find(|item| item.get("key").and_then(Value::as_str) == Some(gateway_key))
        .ok_or_else(|| {
            "未在 Sub2API 中精确匹配当前网关 API key；拒绝按名称猜测生产分组，未修改路由"
                .to_string()
        })?;
    let id = binding
        .get("id")
        .and_then(Value::as_i64)
        .filter(|id| *id > 0)
        .ok_or_else(|| "当前网关 API key 缺少有效 id".to_string())?;
    Ok(GatewayKeyBinding {
        id,
        group_id: binding.get("group_id").and_then(Value::as_i64),
        status: binding
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
    })
}

fn gateway_key_binding() -> Result<GatewayKeyBinding, String> {
    let gateway_key = read_gateway_key()?;
    let body = admin_get("/api/v1/keys?page=1&page_size=100")?;
    let items = body
        .pointer("/data/items")
        .and_then(Value::as_array)
        .or_else(|| response_data(&body).as_array())
        .ok_or_else(|| "Sub2API API key 列表格式无效".to_string())?;
    exact_gateway_key_binding(items, &gateway_key)
}

fn list_openai_groups(include_inactive: bool) -> Result<Vec<Value>, String> {
    let path = if include_inactive {
        "/api/v1/admin/groups/all?include_inactive=true"
    } else {
        "/api/v1/admin/groups/all?platform=openai"
    };
    let body = admin_get(path)?;
    let groups = response_data(&body)
        .as_array()
        .cloned()
        .ok_or_else(|| "Sub2API 分组列表格式无效".to_string())?;
    Ok(groups
        .into_iter()
        .filter(|group| group.get("platform").and_then(Value::as_str) == Some("openai"))
        .collect())
}

fn validate_openai_oauth_account(account_id: i64) -> Result<Value, String> {
    if account_id <= 0 {
        return Err("无效的 account_id".into());
    }
    let body = admin_get(&format!("/api/v1/admin/accounts/{account_id}"))?;
    let account = response_data(&body).clone();
    let platform = account
        .get("platform")
        .and_then(Value::as_str)
        .unwrap_or("");
    let account_type = account.get("type").and_then(Value::as_str).unwrap_or("");
    if platform != "openai" || account_type != "oauth" {
        return Err(format!(
            "只能选择 OpenAI OAuth 账号（当前 platform={platform}, type={account_type}）"
        ));
    }
    Ok(account)
}

pub(crate) fn refresh_managed_route_after_pool_change() -> Result<(), String> {
    let _guard = ROUTING_MUTATION.lock();
    let preferred_account_id = match lookup_routing_preference()? {
        RoutingPreference::Managed(account_id) => account_id,
        RoutingPreference::Unconfigured | RoutingPreference::Stale => return Ok(()),
    };
    let gateway_key = gateway_key_binding()?;
    if gateway_key.status != "active" {
        return Err("当前网关 API key 未启用，无法同步新账号路由".into());
    }
    let group_id = gateway_key
        .group_id
        .filter(|group_id| *group_id > 0)
        .ok_or_else(|| "当前网关 API key 未绑定有效分组".to_string())?;
    let policy = read_routing_policy();
    let openai_accounts = list_openai_accounts()?;
    apply_and_activate_routing_priorities(
        &openai_accounts,
        group_id,
        preferred_account_id,
        policy,
    )?;
    write_routing_preference(preferred_account_id)?;
    write_routing_policy(policy)?;
    Ok(())
}

fn lookup_routing_preference() -> Result<RoutingPreference, String> {
    if let Some(preference) = read_stored_routing_preference() {
        return Ok(preference);
    }
    let binding = gateway_key_binding()?;
    let Some(group_id) = binding.group_id.filter(|group_id| *group_id > 0) else {
        return Ok(RoutingPreference::Unconfigured);
    };
    let groups = list_openai_groups(true)?;
    let group = groups
        .iter()
        .find(|group| group.get("id").and_then(Value::as_i64) == Some(group_id))
        .ok_or_else(|| format!("当前网关分组 #{group_id} 不存在"))?;
    let name = group.get("name").and_then(Value::as_str).unwrap_or("");
    if name.starts_with(AUTOMATIC_GROUP_PREFIX) {
        return Ok(RoutingPreference::Managed(None));
    }
    if !name.starts_with(PREFERRED_OAUTH_GROUP_PREFIX) {
        return Ok(RoutingPreference::Unconfigured);
    }
    match parse_preferred_account_id(name) {
        Some(account_id) => Ok(RoutingPreference::Managed(Some(account_id))),
        None => Ok(RoutingPreference::Stale),
    }
}

fn routing_status_from_preference(
    preference: RoutingPreference,
    accounts: &[Sub2ApiAccountQuota],
    fallback_available: bool,
    auto_pause_threshold_percent: u8,
    policy: RoutingPolicy,
) -> Sub2ApiRoutingStatus {
    let status =
        |preferred_account_id: Option<i64>, state: &str, message: String| Sub2ApiRoutingStatus {
            preferred_account_id,
            state: state.into(),
            message,
            auto_pause_threshold_percent,
            policy: policy.as_str().into(),
            policy_configured: false,
            recent_window_minutes: RECENT_ROUTE_WINDOW_MINUTES as u32,
            recent_request_limit: RECENT_ROUTE_REQUEST_LIMIT,
            recent_request_count: 0,
            last_successful_account_id: None,
            last_successful_account_name: None,
            last_successful_account_type: None,
            last_successful_at: None,
            distribution: Vec::new(),
            oauth_available_count: 0,
            relay_available_count: 0,
            policy_deviation: false,
            policy_deviation_message: None,
            active_relay_name: None,
        };
    match preference {
        RoutingPreference::Unconfigured | RoutingPreference::Managed(None) => status(
            None,
            "automatic",
            "未固定 OAuth 首选账号；Sub2API 按当前策略自动调度。".into(),
        ),
        RoutingPreference::Stale => status(
            None,
            if fallback_available {
                "failover"
            } else {
                "unavailable"
            },
            "当前 Hub 路由标记已失效；仍按组内可用账号自动调度。".into(),
        ),
        RoutingPreference::Managed(Some(preferred_account_id)) => {
            let Some(preferred) = accounts
                .iter()
                .find(|account| account.id == preferred_account_id)
            else {
                return status(
                    None,
                    if fallback_available {
                        "failover"
                    } else {
                        "unavailable"
                    },
                    "保存的 OAuth 首选账号不属于当前生产分组；组内备用账号仍可自动接管。".into(),
                );
            };

            if preferred.available {
                status(
                    Some(preferred_account_id),
                    "preferred",
                    format!(
                        "OAuth 首选账号 {} 可用；实际流量见最近请求分布。",
                        preferred.name
                    ),
                )
            } else if fallback_available {
                status(
                    Some(preferred_account_id),
                    "failover",
                    format!(
                        "OAuth 首选账号 {} 当前不可用；Sub2API 将由组内备用账号接管。",
                        preferred.name
                    ),
                )
            } else {
                status(
                    Some(preferred_account_id),
                    "unavailable",
                    format!(
                        "OAuth 首选账号 {} 与组内备用账号当前都不可用。",
                        preferred.name
                    ),
                )
            }
        }
    }
}

fn fetch_routing_status(
    preference: Result<RoutingPreference, String>,
    accounts: &[Sub2ApiAccountQuota],
    fallback_available: bool,
    auto_pause_threshold_percent: u8,
    policy: RoutingPolicy,
) -> Sub2ApiRoutingStatus {
    match preference {
        Ok(preference) => routing_status_from_preference(
            preference,
            accounts,
            fallback_available,
            auto_pause_threshold_percent,
            policy,
        ),
        Err(error) => Sub2ApiRoutingStatus {
            preferred_account_id: None,
            state: "error".into(),
            message: format!("无法读取当前路由状态：{}", safe_reason(&error, "未知错误")),
            auto_pause_threshold_percent,
            policy: policy.as_str().into(),
            policy_configured: false,
            recent_window_minutes: RECENT_ROUTE_WINDOW_MINUTES as u32,
            recent_request_limit: RECENT_ROUTE_REQUEST_LIMIT,
            recent_request_count: 0,
            last_successful_account_id: None,
            last_successful_account_name: None,
            last_successful_account_type: None,
            last_successful_at: None,
            distribution: Vec::new(),
            oauth_available_count: 0,
            relay_available_count: 0,
            policy_deviation: false,
            policy_deviation_message: None,
            active_relay_name: None,
        },
    }
}

fn fetch_recent_routing_observation(
    gateway_key_id: i64,
    current_group_id: i64,
    cutoff: DateTime<Utc>,
    accounts: &[Value],
    now: DateTime<Utc>,
) -> Result<RoutingObservation, String> {
    let body = admin_get(&format!(
        "/api/v1/admin/usage?page=1&page_size={RECENT_ROUTE_REQUEST_LIMIT}&api_key_id={gateway_key_id}&group_id={current_group_id}&sort_by=created_at&sort_order=desc"
    ))?;
    routing_observation_from_usage(&body, accounts, current_group_id, cutoff, now)
}

fn routing_observation_from_usage(
    body: &Value,
    accounts: &[Value],
    current_group_id: i64,
    cutoff: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Result<RoutingObservation, String> {
    let items = body
        .pointer("/data/items")
        .and_then(Value::as_array)
        .ok_or_else(|| "Sub2API 用量记录格式无效".to_string())?;
    let account_types: HashMap<i64, String> = accounts
        .iter()
        .filter_map(|account| {
            Some((
                account.get("id")?.as_i64()?,
                account.get("type")?.as_str()?.to_string(),
            ))
        })
        .collect();

    let mut observation = RoutingObservation::default();
    let mut counts: HashMap<i64, (String, String, u32)> = HashMap::new();
    for item in items.iter().take(RECENT_ROUTE_REQUEST_LIMIT as usize) {
        let item_group_id = item
            .get("group_id")
            .and_then(Value::as_i64)
            .or_else(|| item.pointer("/group/id").and_then(Value::as_i64));
        if item_group_id != Some(current_group_id) {
            continue;
        }
        let Some(created_at) = item.get("created_at").and_then(parse_timestamp) else {
            continue;
        };
        if created_at < cutoff || created_at > now + chrono::Duration::minutes(1) {
            continue;
        }
        let Some(account_id) = item.get("account_id").and_then(Value::as_i64) else {
            continue;
        };
        let name = item
            .pointer("/account/name")
            .and_then(Value::as_str)
            .map(|name| safe_reason(name, "未知账号"))
            .unwrap_or_else(|| format!("账号 #{account_id}"));
        let account_type = account_types
            .get(&account_id)
            .cloned()
            .unwrap_or_else(|| "unknown".into());
        if observation.last_successful_account_id.is_none() {
            observation.last_successful_account_id = Some(account_id);
            observation.last_successful_account_name = Some(name.clone());
            observation.last_successful_account_type = Some(account_type.clone());
            observation.last_successful_at = Some(created_at.to_rfc3339());
        }
        let entry = counts.entry(account_id).or_insert((name, account_type, 0));
        entry.2 += 1;
    }

    observation.recent_request_count = counts.values().map(|entry| entry.2).sum();
    if observation.recent_request_count > 0 {
        observation.distribution = counts
            .into_iter()
            .map(
                |(account_id, (name, account_type, request_count))| Sub2ApiRoutingDistribution {
                    account_id,
                    name,
                    account_type,
                    request_count,
                    percent: request_count as f64 * 100.0 / observation.recent_request_count as f64,
                },
            )
            .collect();
        observation.distribution.sort_by(|left, right| {
            right
                .request_count
                .cmp(&left.request_count)
                .then(left.name.cmp(&right.name))
        });
    }
    Ok(observation)
}

fn apply_routing_observation(routing: &mut Sub2ApiRoutingStatus, observation: RoutingObservation) {
    routing.recent_request_count = observation.recent_request_count;
    routing.last_successful_account_id = observation.last_successful_account_id;
    routing.last_successful_account_name = observation.last_successful_account_name;
    routing.last_successful_account_type = observation.last_successful_account_type;
    routing.last_successful_at = observation.last_successful_at;
    routing.distribution = observation.distribution;
}

fn is_relay_account_type(account_type: &str) -> bool {
    matches!(
        account_type.trim().to_ascii_lowercase().as_str(),
        "apikey" | "api_key" | "relay"
    )
}

fn evaluate_observed_policy(
    routing: &mut Sub2ApiRoutingStatus,
    policy: RoutingPolicy,
    policy_configured: bool,
    oauth_available_count: u32,
    relay_available_count: u32,
) {
    routing.oauth_available_count = oauth_available_count;
    routing.relay_available_count = relay_available_count;
    routing.policy_deviation = false;
    routing.policy_deviation_message = None;

    // “当前由 … 代跑” must be grounded in the newest successful request,
    // never inferred from an older relay merely dominating the sample window.
    routing.active_relay_name = routing
        .last_successful_account_type
        .as_deref()
        .filter(|kind| is_relay_account_type(kind))
        .and(routing.last_successful_account_name.clone());

    if relay_available_count == 0 {
        if !matches!(
            routing.state.as_str(),
            "error" | "unconfigured" | "unavailable"
        ) {
            routing.state = "fallback_missing".into();
        }
        routing.message.push_str(
            " 生产分组当前没有可用的 apikey 中转兜底；OAuth 后续若全部失效可能返回 503，请运行路由医生。",
        );
    }

    if !policy_configured {
        return;
    }

    // A few weighted samples can legitimately land on the lower-priority tier.
    // Three or more requests with the opposite tier dominating is actionable.
    if routing.recent_request_count < 3 {
        return;
    }
    let (oauth_requests, relay_requests) =
        routing
            .distribution
            .iter()
            .fold((0_u32, 0_u32), |(oauth, relay), entry| {
                if is_relay_account_type(&entry.account_type) {
                    (oauth, relay + entry.request_count)
                } else if entry.account_type.eq_ignore_ascii_case("oauth") {
                    (oauth + entry.request_count, relay)
                } else {
                    (oauth, relay)
                }
            });

    let deviation = match policy {
        RoutingPolicy::OauthFirst if oauth_available_count > 0 && relay_requests > oauth_requests => {
            Some(format!(
                "观察到路由偏离：当前有 {oauth_available_count} 个 OAuth 可用，但最近样本中转 {relay_requests} 次、OAuth {oauth_requests} 次。这不等于配置未生效；请继续核验样本时段与目标模型当时的可用性。"
            ))
        }
        RoutingPolicy::RelayFirst
            if relay_available_count > 0 && oauth_requests > relay_requests =>
        {
            Some(format!(
                "观察到路由偏离：当前有 {relay_available_count} 个中转可用，但最近样本 OAuth {oauth_requests} 次、中转 {relay_requests} 次。这不等于配置未生效；请继续核验样本时段与目标模型当时的可用性。"
            ))
        }
        _ => None,
    };
    if let Some(message) = deviation {
        routing.policy_deviation = true;
        routing.policy_deviation_message = Some(message);
    }
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
    let metadata =
        fs::metadata(path).map_err(|e| format!("无法读取导入文件 {}: {e}", path.display()))?;
    if !metadata.is_file() {
        return Err("导入路径不是普通文件".into());
    }
    if metadata.len() == 0 {
        return Err("导入文件为空".into());
    }
    if metadata.len() > MAX_IMPORT_BYTES {
        return Err(format!(
            "导入文件超过 {} MiB 限制",
            MAX_IMPORT_BYTES / 1024 / 1024
        ));
    }
    Ok(())
}

fn private_import_copy(source: &Path) -> Result<PathBuf, String> {
    validate_import_path(source)?;
    let suffix = source
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("json");
    let target =
        std::env::temp_dir().join(format!("codex-provider-hub-{}.{}", Uuid::new_v4(), suffix));
    let mut source_file = fs::File::open(source).map_err(|e| format!("读取导入文件失败: {e}"))?;
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut target_file = options
        .open(&target)
        .map_err(|e| format!("创建本地导入副本失败: {e}"))?;
    if let Err(error) =
        std::io::copy(&mut source_file, &mut target_file).and_then(|_| target_file.sync_all())
    {
        drop(target_file);
        let _ = fs::remove_file(&target);
        return Err(format!("写入本地导入副本失败: {error}"));
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
        if clean.contains("access_token")
            || clean.contains("refresh_token")
            || clean.contains("Bearer ")
        {
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
pub fn import_sub2api_file(
    file_path: String,
    name: Option<String>,
) -> Result<Sub2ApiImportResult, String> {
    let source = PathBuf::from(file_path);
    let temp_copy = private_import_copy(&source)?;
    let script = sub2api_dir().join("sub2api");
    if !script.is_file() {
        let _ = fs::remove_file(&temp_copy);
        return Err(format!("未找到 Sub2API 导入脚本: {}", script.display()));
    }

    // GUI apps launch with a minimal PATH (/usr/bin:/bin:...), but the
    // importer shells out to docker/jq from Homebrew. Extend the child PATH
    // or every import dies with "[ERROR] 缺少命令：docker".
    let child_path = {
        let mut p = std::env::var("PATH").unwrap_or_default();
        for extra in ["/opt/homebrew/bin", "/usr/local/bin"] {
            if !p.split(':').any(|seg| seg == extra) {
                p.push(':');
                p.push_str(extra);
            }
        }
        p
    };
    let token = admin_login().map_err(|error| {
        let _ = fs::remove_file(&temp_copy);
        format!("准备 Sub2API 管理会话失败: {error}")
    })?;
    let import_name = name
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("");
    // Source the maintained importer, then replace its login helper with a
    // no-op. The cached JWT travels over stdin so it never appears in argv,
    // process environment, logs, or the UI.
    let shell = r#"
source "$1"
load_env
IFS= read -r ADMIN_JWT
admin_login() { :; }
ensure_runtime
wait_for_health
import_file "$2" "$3"
"#;
    let mut child = Command::new("/bin/bash")
        .arg("-c")
        .arg(shell)
        .arg("codex-provider-hub-import")
        .arg(&script)
        .arg(&temp_copy)
        .arg(import_name)
        .env("PATH", child_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            let _ = fs::remove_file(&temp_copy);
            format!("运行 Sub2API 导入器失败: {e}")
        })?;
    if let Some(mut stdin) = child.stdin.take() {
        if let Err(error) = writeln!(stdin, "{token}") {
            let _ = child.kill();
            let _ = child.wait();
            let _ = fs::remove_file(&temp_copy);
            return Err(format!("传递 Sub2API 管理会话失败: {error}"));
        }
    }
    let command_result = child.wait_with_output();
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
            .find(|line| {
                line.contains("[ERROR]") || line.contains("不支持") || line.contains("失败")
            })
            .map(str::trim)
            .unwrap_or("Sub2API 未接受该导入文件");
        return Err(format!(
            "导入失败: {}",
            safe_reason(safe_message, "Sub2API 未接受该导入文件")
        ));
    }
    // Self-heal: imported OAuth accounts need prefixed-model mappings,
    // otherwise OpenAI rejects catalog slugs and the pool 503s.
    let healed = heal_oauth_account_mappings();
    let mut result = result;
    if healed > 0 {
        result.summary = format!("导入完成，并为 OAuth 账号补齐 {healed} 条模型映射。");
    }
    if let Err(error) = refresh_managed_route_after_pool_change() {
        result.summary.push_str(&format!(
            " 路由组同步失败：{}",
            safe_reason(&error, "未知错误")
        ));
    }
    crate::http_util::invalidate_cache("sub2api_usage");
    Ok(result)
}

fn browser_login_url() -> String {
    format!("{GATEWAY_BASE}/admin/accounts")
}

fn start_oauth_callback_listener() -> Result<(), String> {
    BROWSER_CALLBACK_LISTENER
        .get_or_try_init(|| {
            let listener = TcpListener::bind("127.0.0.1:1455")
                .map_err(|e| format!("无法监听 OpenAI OAuth 回调端口 1455: {e}"))?;
            thread::spawn(move || {
                for stream in listener.incoming() {
                    let Ok(mut stream) = stream else { continue };
            let mut request = [0_u8; 8192];
            let Ok(read) = stream.read(&mut request) else {
                        continue;
            };
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
                        let callback_url = format!("http://localhost:1455{path}");
                        let callback_state = callback_query_value(&callback_url, "state");
                if let Some(session) = BROWSER_LOGIN.lock().as_mut() {
                            if callback_state.as_deref() == Some(session.state.as_str()) {
                                session.callback_url = Some(callback_url);
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
            Ok::<(), String>(())
        })
        .map(|_| ())
}

/// Starts a user-driven login handoff. The Hub intentionally opens the local
/// Sub2API account UI rather than collecting an OpenAI password or 2FA code.
#[tauri::command]
pub fn begin_sub2api_browser_login() -> Result<BrowserLoginStatus, String> {
    let response = browser_admin_post("/api/v1/admin/openai/generate-auth-url", json!({}))?;
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
    if let Err(error) = start_oauth_callback_listener() {
        *BROWSER_LOGIN.lock() = None;
        return Err(error);
    }
    Ok(BrowserLoginStatus {
        session_id: Some(id),
        login_url,
        state: "waiting".into(),
        message: "已打开 OpenAI 官方授权页。请在系统浏览器完成登录、2FA 和验证码，回调后 Hub 会自动导入账号。".into(),
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
        message: "请在浏览器完成登录和 2FA；Hub 正在等待本机 OAuth 回调并会自动导入。".into(),
        imported_accounts: vec![],
    })
}

fn callback_query_value(callback_url: &str, key: &str) -> Option<String> {
    let query = callback_url
        .split_once('?')?
        .1
        .split('#')
        .next()
        .unwrap_or("");
    query.split('&').find_map(|part| {
        let (name, value) = part.split_once('=')?;
        if name == key {
            urlencoding::decode(value).ok().map(|s| s.into_owned())
        } else {
            None
        }
    })
}

/// Completes the server-issued OAuth session using the callback received from
/// the system browser after OpenAI login and 2FA.
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
    let response = browser_admin_post("/api/v1/admin/openai/create-from-oauth", payload)?;
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
    let mut message = if mapped > 0 {
        format!("OpenAI/Codex OAuth 账号已导入，并配置 {mapped} 条模型映射。")
    } else {
        "OpenAI/Codex OAuth 账号已导入。".to_string()
    };
    if let Err(error) = refresh_managed_route_after_pool_change() {
        message.push_str(&format!(
            " 路由组同步失败：{}",
            safe_reason(&error, "未知错误")
        ));
    }
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
    if guard
        .as_ref()
        .is_some_and(|session| session.id == session_id)
    {
        *guard = None;
        return Ok(());
    }
    Err("浏览器登录会话不匹配或已结束".into())
}

pub fn fetch_sub2api_usage() -> Result<Sub2ApiUsage, String> {
    let raw_openai_accounts = list_openai_accounts()?;
    let raw_accounts: Vec<Value> = raw_openai_accounts
        .iter()
        .filter(|account| account.get("type").and_then(Value::as_str) == Some("oauth"))
        .cloned()
        .collect();
    let mut accounts: Vec<Sub2ApiAccountQuota> = Vec::with_capacity(raw_accounts.len());
    let now = Utc::now();
    let binding = gateway_key_binding()?;
    let current_group_id = binding
        .group_id
        .filter(|group_id| *group_id > 0)
        .ok_or_else(|| "当前生产网关 API key 未绑定有效分组".to_string())?;
    let current_group_accounts =
        resolve_openai_accounts_for_group(&raw_openai_accounts, current_group_id)?;
    let current_group_account_ids = current_group_accounts
        .iter()
        .filter_map(|account| account.get("id").and_then(Value::as_i64))
        .collect::<HashSet<_>>();

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
        let account_type = a
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        let raw_status = a.get("status").and_then(|v| v.as_str()).unwrap_or("");
        let raw_error_message = a
            .get("error_message")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let error_message = if raw_error_message.trim().is_empty() {
            String::new()
        } else {
            safe_reason(raw_error_message, "账号状态异常")
        };
        let status = normalize_status(raw_status, raw_error_message);
        let schedulable = a
            .get("schedulable")
            .and_then(|v| v.as_bool())
            .unwrap_or(status == "ready");
        let availability = classify_account_availability(&a, now);

        let (five_hour, seven_day) = if id > 0 {
            fetch_account_usage(id)
        } else {
            (None, None)
        };

        // Prefer usage endpoint; fall back to account.extra codex_* fields if present.
        // Skip the fallback when the account reports no such window at all
        // (window_minutes == 0), otherwise phantom 100% / reset 0 windows show up.
        // extra semantics: primary window = 7d, secondary window = 5h.
        let window_minutes = |key: &str| {
            a.pointer(&format!("/extra/{key}"))
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0)
        };
        let five_hour = five_hour.or_else(|| {
            if window_minutes("codex_5h_window_minutes") <= 0.0
                && window_minutes("codex_secondary_window_minutes") <= 0.0
            {
                return None;
            }
            let used = a
                .pointer("/extra/codex_5h_used_percent")
                .and_then(|v| v.as_f64())?;
            Some(QuotaWindow {
                remaining_percent: (100.0 - used).clamp(0.0, 100.0),
                reset_after_seconds: window_minutes("codex_5h_reset_after_seconds") as u64,
            })
        });
        let seven_day = seven_day.or_else(|| {
            if window_minutes("codex_7d_window_minutes") <= 0.0
                && window_minutes("codex_primary_window_minutes") <= 0.0
            {
                return None;
            }
            let used = a
                .pointer("/extra/codex_7d_used_percent")
                .and_then(|v| v.as_f64())?;
            Some(QuotaWindow {
                remaining_percent: (100.0 - used).clamp(0.0, 100.0),
                reset_after_seconds: window_minutes("codex_7d_reset_after_seconds") as u64,
            })
        });

        accounts.push(Sub2ApiAccountQuota {
            id,
            name,
            email,
            account_type,
            status,
            error_message,
            five_hour,
            seven_day,
            schedulable,
            available: availability.available,
            availability: availability.availability,
            availability_reason: availability.reason,
            recoverable: availability.recoverable,
            unavailable_until: availability.unavailable_until,
            preferred: false,
        });
    }

    let policy_path_exists = routing_policy_path().is_file();
    let stored_policy = stored_routing_policy();
    let policy = stored_policy.unwrap_or_else(read_routing_policy);
    let preference = lookup_routing_preference();
    let preferred_account_id = match preference.as_ref().ok().copied() {
        Some(RoutingPreference::Managed(account_id)) => account_id,
        _ => None,
    };
    let (fallback_available, relay_available_count, oauth_available_count) =
        routing_availability_for_group(
            &current_group_accounts,
            current_group_id,
            preferred_account_id,
            now,
        );
    let current_group_oauth_accounts = accounts
        .iter()
        .filter(|account| current_group_account_ids.contains(&account.id))
        .cloned()
        .collect::<Vec<_>>();
    let policy_priorities_match = routing_policy_matches_accounts(
        &current_group_accounts,
        current_group_id,
        preferred_account_id,
        policy,
    );
    let policy_configured = stored_policy.is_some() && policy_priorities_match;

    let (auto_pause_threshold_percent, threshold_error) =
        match load_or_initialize_auto_pause_threshold() {
            Ok(percent) => (percent, None),
            Err(error) => (100, Some(error)),
        };
    let mut routing = fetch_routing_status(
        preference,
        &current_group_oauth_accounts,
        fallback_available,
        auto_pause_threshold_percent,
        policy,
    );
    routing.policy_configured = policy_configured;
    if !policy_configured {
        routing.state = "unconfigured".into();
        routing.message = if !policy_path_exists {
            "Hub 路由策略尚未应用；当前仍按 Sub2API 现有优先级运行，请选择策略后再按实际流量验收。"
                .into()
        } else if stored_policy.is_none() {
            "Hub 路由策略状态文件无效；当前生产组优先级不能视为已应用，请重新选择策略。".into()
        } else {
            format!(
                "Hub 路由策略状态与生产组 #{current_group_id} 的 accounts.priority 不一致；可能被外部修改，或上次应用只完成了一部分，请重新应用后再按实际流量验收。"
            )
        };
    }
    if let Some(error) = threshold_error {
        routing.message.push_str(&format!(
            " 自动暂停阈值读取失败：{}",
            safe_reason(&error, "未知错误")
        ));
    }
    let observation_cutoff = recent_routing_cutoff(now, routing_policy_modified_at());
    match fetch_recent_routing_observation(
        binding.id,
        current_group_id,
        observation_cutoff,
        &current_group_accounts,
        now,
    ) {
        Ok(observation) => apply_routing_observation(&mut routing, observation),
        Err(error) => routing.message.push_str(&format!(
            " 最近请求分布读取失败：{}",
            safe_reason(&error, "未知错误")
        )),
    }
    evaluate_observed_policy(
        &mut routing,
        policy,
        policy_configured,
        oauth_available_count,
        relay_available_count,
    );
    if let Some(preferred_account_id) = routing.preferred_account_id {
        for account in &mut accounts {
            account.preferred = account.id == preferred_account_id;
        }
    }

    accounts.sort_by(|a, b| {
        // Keep the selected account visible, then schedulable accounts, then
        // stable status/name ordering.
        let rank = |s: &str| match s {
            "ready" => 0,
            "inactive" => 1,
            "error" => 2,
            _ => 3,
        };
        b.preferred
            .cmp(&a.preferred)
            .then(b.available.cmp(&a.available))
            .then(
                rank(&a.status)
                    .cmp(&rank(&b.status))
                    .then(a.name.cmp(&b.name)),
            )
    });

    let pool_total = accounts.len() as u32;
    let pool_available = accounts.iter().filter(|a| a.available).count() as u32;

    let five_windows: Vec<_> = accounts
        .iter()
        .filter(|account| account.available)
        .filter_map(|a| a.five_hour.clone())
        .collect();
    let seven_windows: Vec<_> = accounts
        .iter()
        .filter(|account| account.available)
        .filter_map(|a| a.seven_day.clone())
        .collect();

    Ok(Sub2ApiUsage {
        five_hour: avg_window(&five_windows),
        seven_day: avg_window(&seven_windows),
        pool_total,
        pool_available,
        accounts,
        routing,
        fetched_at: now_iso(),
    })
}

/// Fetch per-account OpenAI/Codex OAuth quotas (excludes apikey 中转站).
#[tauri::command]
pub fn get_sub2api_usage() -> Result<Sub2ApiUsage, String> {
    crate::http_util::cached_json(
        "sub2api_usage",
        Duration::from_secs(30),
        fetch_sub2api_usage,
    )
}

/// Prefer one OpenAI OAuth account while keeping every other OAuth account in
/// the managed group as a native Sub2API failover candidate.
#[tauri::command]
pub fn set_sub2api_current_account(account_id: i64) -> Result<Sub2ApiUsage, String> {
    let _guard = ROUTING_MUTATION.lock();
    let account = validate_openai_oauth_account(account_id)?;
    let availability = classify_account_availability(&account, Utc::now());
    if !availability.available {
        return Err(format!("该 OAuth 账号当前不可用：{}", availability.reason));
    }

    // Resolve and validate the live gateway key before any mutation. The raw
    // key is compared in memory only; this sanitized binding is all we retain.
    let gateway_key = gateway_key_binding()?;
    if gateway_key.status != "active" {
        return Err("当前网关 API key 未启用，无法切换账号".into());
    }
    let group_id = gateway_key
        .group_id
        .filter(|group_id| *group_id > 0)
        .ok_or_else(|| "当前网关 API key 未绑定有效分组".to_string())?;
    if !group_ids_from_account(&account).contains(&group_id) {
        return Err(format!(
            "所选 OAuth 账号 #{account_id} 不属于当前生产分组 #{group_id}，未修改路由"
        ));
    }

    let policy = read_routing_policy();
    let openai_accounts = list_openai_accounts()?;
    apply_and_activate_routing_priorities(&openai_accounts, group_id, Some(account_id), policy)?;
    write_routing_preference(Some(account_id))?;
    write_routing_policy(policy)?;

    crate::http_util::invalidate_cache("sub2api_usage");
    fetch_sub2api_usage()
}

/// Clear recoverable runtime state through Sub2API's native recovery endpoint.
#[tauri::command]
pub fn recover_sub2api_account(account_id: i64) -> Result<Sub2ApiUsage, String> {
    validate_openai_oauth_account(account_id)?;
    admin_post(
        &format!("/api/v1/admin/accounts/{account_id}/recover-state"),
        json!({}),
    )?;
    crate::http_util::invalidate_cache("sub2api_usage");
    fetch_sub2api_usage()
}

/// Switch the relative priority of OAuth accounts and API-key relays while
/// leaving the production API key on its current group. Relay accounts remain
/// schedulable so they can take over when every OAuth account is exhausted.
#[tauri::command]
pub fn set_sub2api_routing_policy(policy: String) -> Result<Sub2ApiUsage, String> {
    let _guard = ROUTING_MUTATION.lock();
    let policy = RoutingPolicy::parse(&policy)?;
    let gateway_key = gateway_key_binding()?;
    if gateway_key.status != "active" {
        return Err("当前网关 API key 未启用，无法切换路由策略".into());
    }
    let group_id = gateway_key
        .group_id
        .filter(|group_id| *group_id > 0)
        .ok_or_else(|| "当前网关 API key 未绑定有效分组".to_string())?;
    let preferred_account_id = match lookup_routing_preference()? {
        RoutingPreference::Managed(account_id) => account_id,
        _ => None,
    };
    let openai_accounts = list_openai_accounts()?;
    apply_and_activate_routing_priorities(
        &openai_accounts,
        group_id,
        preferred_account_id,
        policy,
    )?;
    write_routing_preference(preferred_account_id)?;
    write_routing_policy(policy)?;
    crate::http_util::invalidate_cache("sub2api_usage");
    fetch_sub2api_usage()
}

/// Set the native OpenAI scheduling threshold. `100` disables proactive
/// auto-pause; `1..=99` pauses at that native usage percentage until reset.
#[tauri::command]
pub fn set_sub2api_auto_pause_threshold(percent: u8) -> Result<Sub2ApiUsage, String> {
    validate_auto_pause_threshold(percent)?;
    let guard = AUTO_PAUSE_INITIALIZATION.lock();
    let settings = admin_get("/api/v1/admin/settings")?;
    // Record explicit user intent before the remote write. If the PUT fails,
    // a later background refresh must not reinterpret the upstream-safe 100%
    // initialization as permission to choose a lower threshold.
    write_auto_pause_sentinel()?;
    admin_put(
        "/api/v1/admin/settings",
        scheduling_thresholds_payload(&settings, percent),
    )?;
    drop(guard);

    crate::http_util::invalidate_cache("sub2api_usage");
    fetch_sub2api_usage()
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
    fn phantom_window_without_time_left_is_no_data() {
        // Sub2API emits this shape when upstream reports no such window.
        let phantom = json!({
            "utilization": 0,
            "resets_at": "2026-08-11T16:06:06+08:00",
            "remaining_seconds": 0
        });
        assert!(window_from_usage(&phantom).is_none());
        let real = json!({ "utilization": 14, "remaining_seconds": 603268 });
        let w = window_from_usage(&real).expect("real window");
        assert_eq!(w.remaining_percent, 86.0);
        assert_eq!(w.reset_after_seconds, 603268);
        assert!(window_from_usage(&Value::Null).is_none());
    }

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

    fn hub_test_now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-08-11T12:00:00Z")
            .expect("timestamp")
            .with_timezone(&Utc)
    }

    fn hub_test_account(id: i64, available: bool) -> Sub2ApiAccountQuota {
        Sub2ApiAccountQuota {
            id,
            name: format!("OAuth {id}"),
            email: format!("oauth-{id}@example.test"),
            account_type: "oauth".into(),
            status: if available { "ready" } else { "error" }.into(),
            error_message: String::new(),
            five_hour: None,
            seven_day: None,
            schedulable: available,
            available,
            availability: if available { "ready" } else { "error" }.into(),
            availability_reason: String::new(),
            recoverable: !available,
            unavailable_until: None,
            preferred: false,
        }
    }

    #[test]
    fn hub_admin_auth_state_reuses_tokens_and_honors_backoff() {
        let now = Instant::now();
        let ready = AdminAuthState::Ready(CachedAdminToken {
            value: "cached-token".into(),
            expires_at: now + Duration::from_secs(1),
        });
        assert_eq!(
            ready.cached_result_at(now).expect("cached token").unwrap(),
            "cached-token"
        );
        assert!(ready
            .cached_result_at(now + Duration::from_secs(1))
            .is_none());

        let backoff = AdminAuthState::Backoff {
            retry_at: now + Duration::from_secs(2),
            error: "admin login HTTP 429".into(),
        };
        assert!(backoff
            .cached_result_at(now)
            .expect("cached failure")
            .is_err());
        assert!(backoff
            .cached_result_at(now + Duration::from_secs(2))
            .is_none());
        assert_eq!(
            safe_reason("Authorization: Bearer secret", "request failed"),
            "request failed"
        );
        assert_eq!(
            admin_error_reason(&json!({"error": {"message": "Bearer secret"}}), "fallback"),
            "fallback"
        );
        assert_eq!(
            admin_error_reason(&json!({"message": "temporarily unavailable"}), "fallback"),
            "temporarily unavailable"
        );
        assert!(is_transient_gateway_connection_error(
            "admin login: error sending request for url"
        ));
        assert!(is_transient_gateway_connection_error("connection refused"));
        assert!(is_transient_gateway_connection_error(
            "admin login: 无法连接"
        ));
        assert!(!is_transient_gateway_connection_error(
            "admin login HTTP 429: too many requests"
        ));
    }

    #[test]
    fn hub_availability_precedence_and_timestamps() {
        let now = hub_test_now();
        let errored = json!({
            "status": "active",
            "error_message": "token revoked",
            "schedulable": false,
            "rate_limit_reset_at": "2026-08-11T14:00:00Z"
        });
        let state = classify_account_availability(&errored, now);
        assert_eq!(state.availability, "error");
        assert!(state.reason.contains("token revoked"));

        let paused = json!({ "status": "active", "schedulable": false });
        assert_eq!(
            classify_account_availability(&paused, now).availability,
            "paused"
        );

        let expired = json!({
            "status": "active",
            "schedulable": true,
            "expires_at": now.timestamp() - 1
        });
        assert_eq!(
            classify_account_availability(&expired, now).availability,
            "expired"
        );

        let ready = json!({
            "status": "active",
            "schedulable": true,
            "expires_at": now.timestamp() + 3600,
            "rate_limit_reset_at": "2026-08-11T11:59:59Z",
            "overload_until": "2026-08-11T11:59:59Z",
            "temp_unschedulable_until": "2026-08-11T11:59:59Z"
        });
        assert!(classify_account_availability(&ready, now).available);
    }

    #[test]
    fn hub_model_rate_limit_keeps_account_available_until_latest_reset() {
        let now = hub_test_now();
        let account = json!({
            "status": "active",
            "schedulable": true,
            "extra": {
                "model_rate_limits": {
                    "past": { "rate_limit_reset_at": "2026-08-11T11:00:00Z" },
                    "later": { "rate_limit_reset_at": "2026-08-11T14:00:00Z" },
                    "earlier": { "rate_limit_reset_at": "2026-08-11T12:30:00Z" }
                }
            }
        });
        let state = classify_account_availability(&account, now);
        assert_eq!(state.availability, "model_rate_limited");
        assert!(state.available);
        assert_eq!(
            state.unavailable_until.as_deref(),
            Some("2026-08-11T14:00:00+00:00")
        );
    }

    #[test]
    fn hub_recoverable_matches_native_runtime_state() {
        let now = hub_test_now();
        let errored = json!({ "status": "error", "schedulable": true });
        assert!(classify_account_availability(&errored, now).recoverable);
        let revoked = json!({ "status": "error", "error_message": "token revoked" });
        assert!(!classify_account_availability(&revoked, now).recoverable);

        let paused = json!({ "status": "active", "schedulable": false });
        assert!(!classify_account_availability(&paused, now).recoverable);

        let expired = json!({
            "status": "active",
            "schedulable": true,
            "expires_at": now.timestamp() - 1
        });
        assert!(!classify_account_availability(&expired, now).recoverable);

        let temporary = json!({
            "status": "active",
            "schedulable": true,
            "temp_unschedulable_until": "2026-08-11T13:00:00Z"
        });
        assert!(classify_account_availability(&temporary, now).recoverable);
    }

    #[test]
    fn hub_preferred_group_marker_round_trips() {
        let name = preferred_group_name(42);
        assert_eq!(name, "codex-provider-hub:preferred-oauth:42");
        assert_eq!(parse_preferred_account_id(&name), Some(42));
        for policy in [
            RoutingPolicy::OauthFirst,
            RoutingPolicy::RelayFirst,
            RoutingPolicy::Balanced,
        ] {
            let routed_name = routing_group_name(Some(42), policy);
            assert_eq!(parse_preferred_account_id(&routed_name), Some(42));
            assert!(routed_name.ends_with(policy.as_str()));
        }
        assert_eq!(
            parse_preferred_account_id(PREFERRED_OAUTH_GROUP_PREFIX),
            None
        );
        assert_eq!(
            parse_preferred_account_id("codex-provider-hub:preferred-oauth:-1"),
            None
        );
        assert_eq!(parse_preferred_account_id("openai-default"), None);
    }

    #[test]
    fn hub_routing_priorities_use_sub2api_lower_number_semantics() {
        assert_eq!(
            desired_account_priority(RoutingPolicy::OauthFirst, "oauth", true),
            0
        );
        assert_eq!(
            desired_account_priority(RoutingPolicy::OauthFirst, "oauth", false),
            10
        );
        assert_eq!(
            desired_account_priority(RoutingPolicy::OauthFirst, "apikey", false),
            100
        );
        assert_eq!(
            desired_account_priority(RoutingPolicy::RelayFirst, "apikey", false),
            0
        );
        assert_eq!(
            desired_account_priority(RoutingPolicy::RelayFirst, "oauth", true),
            50
        );
        assert_eq!(
            desired_account_priority(RoutingPolicy::Balanced, "oauth", false),
            50
        );
        assert_eq!(
            desired_account_priority(RoutingPolicy::Balanced, "apikey", false),
            50
        );
        assert_eq!(
            group_ids_from_account(&json!({ "group_ids": [2, 0, null, 9] })),
            vec![2, 9]
        );
    }

    #[test]
    fn hub_routing_plan_requires_a_ready_relay_and_preserves_originals() {
        let now = hub_test_now();
        let members = vec![
            json!({
                "id": 2,
                "type": "apikey",
                "status": "active",
                "schedulable": true,
                "priority": 50
            }),
            json!({
                "id": 7,
                "type": "oauth",
                "status": "active",
                "schedulable": true,
                "priority": 10
            }),
        ];
        let plan =
            plan_routing_priority_changes(&members, 2, Some(7), RoutingPolicy::OauthFirst, now)
                .expect("safe routing plan");
        assert_eq!(
            plan,
            vec![
                RoutingPriorityChange {
                    account_id: 2,
                    original_priority: 50,
                    desired_priority: 100,
                },
                RoutingPriorityChange {
                    account_id: 7,
                    original_priority: 10,
                    desired_priority: 0,
                },
            ]
        );

        let no_relay = plan_routing_priority_changes(
            &members[1..],
            2,
            Some(7),
            RoutingPolicy::OauthFirst,
            now,
        )
        .expect_err("must refuse a production group without relay fallback");
        assert!(no_relay.contains("apikey 中转兜底"));

        let cooling_relay = vec![json!({
            "id": 2,
            "type": "apikey",
            "status": "active",
            "schedulable": true,
            "priority": 50,
            "rate_limit_reset_at": "2026-08-11T13:00:00Z"
        })];
        assert!(plan_routing_priority_changes(
            &cooling_relay,
            2,
            None,
            RoutingPolicy::Balanced,
            now,
        )
        .is_err());
    }

    #[test]
    fn hub_gateway_binding_never_falls_back_to_a_matching_name() {
        let items = vec![json!({
            "id": 1,
            "key": "different-secret",
            "name": "json-direct-proxy",
            "group_id": 9,
            "status": "active"
        })];
        let error = exact_gateway_key_binding(&items, "configured-secret")
            .expect_err("name-only match must be rejected");
        assert!(error.contains("精确匹配"));

        let binding =
            exact_gateway_key_binding(&items, "different-secret").expect("exact key match");
        assert_eq!(binding.id, 1);
        assert_eq!(binding.group_id, Some(9));
    }

    #[test]
    fn hub_policy_configured_requires_every_current_group_db_priority() {
        let correct = vec![
            json!({ "id": 2, "type": "apikey", "priority": 100, "group_ids": [2] }),
            json!({ "id": 7, "type": "oauth", "priority": 0, "group_ids": [2] }),
            json!({ "id": 8, "type": "oauth", "priority": 10, "group_ids": [2] }),
            json!({ "id": 3, "type": "apikey", "priority": 0, "group_ids": [9] }),
        ];
        assert!(routing_policy_matches_accounts(
            &correct,
            2,
            Some(7),
            RoutingPolicy::OauthFirst,
        ));

        let mut externally_modified = correct.clone();
        externally_modified[0]["priority"] = json!(50);
        assert!(!routing_policy_matches_accounts(
            &externally_modified,
            2,
            Some(7),
            RoutingPolicy::OauthFirst,
        ));

        let mut half_applied = correct.clone();
        half_applied[2].as_object_mut().unwrap().remove("priority");
        assert!(!routing_policy_matches_accounts(
            &half_applied,
            2,
            Some(7),
            RoutingPolicy::OauthFirst,
        ));
        assert!(!routing_policy_matches_accounts(
            &correct,
            2,
            Some(99),
            RoutingPolicy::OauthFirst,
        ));
    }

    #[test]
    fn hub_routing_availability_ignores_accounts_outside_production_group() {
        let now = hub_test_now();
        let accounts = vec![
            json!({ "id": 7, "type": "oauth", "status": "active", "schedulable": true, "group_ids": [2] }),
            json!({ "id": 2, "type": "apikey", "status": "active", "schedulable": true, "group_ids": [2] }),
            json!({ "id": 3, "type": "apikey", "status": "active", "schedulable": true, "group_ids": [9] }),
            json!({ "id": 9, "type": "oauth", "status": "active", "schedulable": true, "group_ids": [9] }),
        ];
        assert_eq!(
            routing_availability_for_group(&accounts, 2, Some(7), now),
            (true, 1, 1)
        );

        let preferred_only_in_group = vec![accounts[0].clone(), accounts[2].clone()];
        assert_eq!(
            routing_availability_for_group(&preferred_only_in_group, 2, Some(7), now),
            (false, 0, 1)
        );
    }

    #[test]
    fn hub_observed_policy_flags_real_dominance_but_not_expected_fallback() {
        let mut routing = routing_status_from_preference(
            RoutingPreference::Unconfigured,
            &[],
            true,
            95,
            RoutingPolicy::OauthFirst,
        );
        routing.recent_request_count = 10;
        routing.last_successful_account_name = Some("AIHub".into());
        routing.last_successful_account_type = Some("apikey".into());
        routing.distribution = vec![
            Sub2ApiRoutingDistribution {
                account_id: 2,
                name: "AIHub".into(),
                account_type: "apikey".into(),
                request_count: 8,
                percent: 80.0,
            },
            Sub2ApiRoutingDistribution {
                account_id: 7,
                name: "OAuth 7".into(),
                account_type: "oauth".into(),
                request_count: 2,
                percent: 20.0,
            },
        ];

        evaluate_observed_policy(&mut routing, RoutingPolicy::OauthFirst, true, 2, 2);
        assert!(routing.policy_deviation);
        assert_eq!(routing.active_relay_name.as_deref(), Some("AIHub"));
        assert!(routing
            .policy_deviation_message
            .as_deref()
            .is_some_and(|message| message.contains("中转 8 次")));

        evaluate_observed_policy(&mut routing, RoutingPolicy::OauthFirst, true, 0, 2);
        assert!(!routing.policy_deviation);
        assert!(routing.policy_deviation_message.is_none());
    }

    #[test]
    fn hub_recent_route_observation_excludes_stale_usage() {
        let now = hub_test_now();
        let policy_cutoff = now - chrono::Duration::minutes(2);
        let body = json!({
            "data": { "items": [
                { "group_id": 2, "account_id": 2, "created_at": "2026-08-11T11:40:00Z", "account": { "name": "old relay" } },
                { "group_id": 2, "account_id": 2, "created_at": "2026-08-11T11:57:00Z", "account": { "name": "pre-policy relay" } },
                { "group_id": 9, "account_id": 2, "created_at": "2026-08-11T11:59:30Z", "account": { "name": "wrong-group relay" } },
                { "group_id": 2, "account_id": 7, "created_at": "2026-08-11T11:59:00Z", "account": { "name": "OAuth 7" } }
            ] }
        });
        let accounts = vec![
            json!({ "id": 2, "type": "apikey" }),
            json!({ "id": 7, "type": "oauth" }),
        ];
        let observation = routing_observation_from_usage(&body, &accounts, 2, policy_cutoff, now)
            .expect("route observation");
        assert_eq!(observation.recent_request_count, 1);
        assert_eq!(observation.last_successful_account_id, Some(7));
        assert_eq!(
            observation.last_successful_at.as_deref(),
            Some("2026-08-11T11:59:00+00:00")
        );
        assert_eq!(
            observation.last_successful_account_name.as_deref(),
            Some("OAuth 7")
        );
        assert_eq!(observation.distribution.len(), 1);
    }

    #[test]
    fn hub_recent_route_observation_is_capped_at_100_records() {
        let now = hub_test_now();
        let items = (0..101)
            .map(|_| {
                json!({
                    "group_id": 2,
                    "account_id": 7,
                    "created_at": "2026-08-11T11:59:00Z",
                    "account": { "name": "OAuth 7" }
                })
            })
            .collect::<Vec<_>>();
        let body = json!({ "data": { "items": items } });
        let accounts = vec![json!({ "id": 7, "type": "oauth" })];
        let observation = routing_observation_from_usage(
            &body,
            &accounts,
            2,
            now - chrono::Duration::minutes(10),
            now,
        )
        .expect("route observation");
        assert_eq!(observation.recent_request_count, RECENT_ROUTE_REQUEST_LIMIT);
    }

    #[test]
    fn hub_recent_route_cutoff_uses_later_policy_timestamp() {
        let now = hub_test_now();
        let policy_modified_at = now - chrono::Duration::minutes(2);
        assert_eq!(
            recent_routing_cutoff(now, Some(policy_modified_at)),
            policy_modified_at
        );
        assert_eq!(
            recent_routing_cutoff(now, Some(now - chrono::Duration::minutes(20))),
            now - chrono::Duration::minutes(RECENT_ROUTE_WINDOW_MINUTES)
        );
    }

    #[test]
    fn hub_routing_states_follow_preference_availability() {
        let threshold = 95;
        let ready = hub_test_account(1, true);
        let unavailable = hub_test_account(1, false);
        let alternative = hub_test_account(2, true);

        assert_eq!(
            routing_status_from_preference(
                RoutingPreference::Unconfigured,
                &[ready.clone()],
                true,
                threshold,
                RoutingPolicy::OauthFirst,
            )
            .state,
            "automatic"
        );
        assert_eq!(
            routing_status_from_preference(
                RoutingPreference::Managed(Some(1)),
                &[ready],
                true,
                threshold,
                RoutingPolicy::OauthFirst,
            )
            .state,
            "preferred"
        );
        assert_eq!(
            routing_status_from_preference(
                RoutingPreference::Managed(Some(1)),
                &[unavailable.clone(), alternative],
                true,
                threshold,
                RoutingPolicy::OauthFirst,
            )
            .state,
            "failover"
        );
        assert_eq!(
            routing_status_from_preference(
                RoutingPreference::Managed(Some(1)),
                &[unavailable],
                false,
                threshold,
                RoutingPolicy::OauthFirst,
            )
            .state,
            "unavailable"
        );
        assert_eq!(
            routing_status_from_preference(
                RoutingPreference::Managed(Some(99)),
                &[],
                false,
                threshold,
                RoutingPolicy::OauthFirst,
            )
            .state,
            "unavailable"
        );
    }

    #[test]
    fn hub_usage_average_uses_mean_remaining_and_earliest_reset() {
        assert!(avg_window(&[]).is_none());
        let average = avg_window(&[
            QuotaWindow {
                remaining_percent: 90.0,
                reset_after_seconds: 600,
            },
            QuotaWindow {
                remaining_percent: 60.0,
                reset_after_seconds: 120,
            },
        ])
        .expect("average window");
        assert_eq!(average.remaining_percent, 75.0);
        assert_eq!(average.reset_after_seconds, 120);
    }

    #[test]
    fn hub_official_quota_classifies_windows_by_duration() {
        let body = json!({
            "plan_type": "k12",
            "rate_limit": {
                "allowed": false,
                "limit_reached": true,
                "primary_window": {
                    "used_percent": 100.0,
                    "limit_window_seconds": 18_000,
                    "reset_after_seconds": 8_192,
                    "reset_at": 1_786_457_273_i64
                },
                "secondary_window": {
                    "used_percent": 18.0,
                    "limit_window_seconds": 604_800,
                    "reset_after_seconds": 594_992,
                    "reset_at": 1_787_044_073_i64
                }
            }
        });
        let probe = parse_official_quota(7, &body).expect("official quota");
        assert_eq!(probe.plan_type, "k12");
        assert!(probe.limit_reached);
        assert!(!probe.allowed);
        let five = probe.five_hour.expect("5h window");
        assert_eq!(five.used_percent, 100.0);
        assert!(five.limit_reached);
        let seven = probe.seven_day.expect("7d window");
        assert_eq!(seven.used_percent, 18.0);
        assert!(!seven.limit_reached);
    }

    #[test]
    fn hub_official_root_limit_does_not_mark_the_wrong_window() {
        let body = json!({
            "plan_type": "plus",
            "rate_limit": {
                "allowed": false,
                "limit_reached": true,
                "primary_window": {
                    "used_percent": 18.0,
                    "limit_window_seconds": 604_800,
                    "reset_after_seconds": 500_000
                },
                "secondary_window": {
                    "used_percent": 100.0,
                    "limit_window_seconds": 18_000,
                    "reset_after_seconds": 4_000
                }
            }
        });
        let probe = parse_official_quota(7, &body).expect("official quota");
        assert!(probe.limit_reached);
        assert!(probe.five_hour.expect("5h window").limit_reached);
        assert!(!probe.seven_day.expect("7d window").limit_reached);
    }

    #[test]
    fn hub_auto_pause_threshold_validation_and_merge() {
        assert_eq!(AUTO_PAUSE_DEFAULT_PERCENT, 100);
        assert!(validate_auto_pause_threshold(0).is_err());
        assert!(validate_auto_pause_threshold(1).is_ok());
        assert!(validate_auto_pause_threshold(99).is_ok());
        assert!(validate_auto_pause_threshold(100).is_ok());

        let settings = json!({
            "data": {
                "account_scheduling_thresholds": {
                    "openai": 100,
                    "anthropic": 88,
                    "grok": 77
                }
            }
        });
        let payload = scheduling_thresholds_payload(&settings, 95);
        assert_eq!(
            payload.pointer("/account_scheduling_thresholds/openai"),
            Some(&json!(95))
        );
        assert_eq!(
            payload.pointer("/account_scheduling_thresholds/anthropic"),
            Some(&json!(88))
        );
        assert_eq!(
            payload.pointer("/account_scheduling_thresholds/grok"),
            Some(&json!(77))
        );
    }

    #[test]
    #[ignore = "requires the live local Sub2API deployment"]
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
                "  #{} status={} 5h={:?} 7d={:?}",
                a.id,
                a.status,
                a.five_hour.as_ref().map(|w| w.remaining_percent),
                a.seven_day.as_ref().map(|w| w.remaining_percent),
            );
        }
        // Relays must not be counted as oauth pool members.
        assert!(u
            .accounts
            .iter()
            .all(|a| !a.name.eq_ignore_ascii_case("AIHub")
                && !a.name.eq_ignore_ascii_case("AnyRouter")));
    }

    #[test]
    #[ignore = "mutates the live local Sub2API deployment"]
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

    #[test]
    #[ignore = "requires live local Sub2API and OpenAI network access"]
    fn live_official_quota_and_routing_inputs() {
        let accounts = list_openai_accounts().expect("OpenAI accounts");
        assert!(!accounts.is_empty(), "live OpenAI account pool is empty");
        for account in &accounts {
            let id = account
                .get("id")
                .and_then(Value::as_i64)
                .expect("account id");
            let detail =
                admin_get(&format!("/api/v1/admin/accounts/{id}")).expect("account detail");
            assert!(
                !group_ids_from_account(response_data(&detail)).is_empty(),
                "account #{id} has no group membership"
            );
        }
        let oauth_id = accounts
            .iter()
            .find(|account| account.get("type").and_then(Value::as_str) == Some("oauth"))
            .and_then(|account| account.get("id"))
            .and_then(Value::as_i64)
            .expect("OAuth account");
        let quota = probe_sub2api_official_quota(oauth_id).expect("official quota probe");
        assert_eq!(quota.account_id, oauth_id);
        assert!(quota.five_hour.is_some() || quota.seven_day.is_some());
    }

    #[test]
    #[ignore = "creates a live OpenAI OAuth browser session without completing it"]
    fn live_browser_login_handoff() {
        let status = begin_sub2api_browser_login().expect("browser login handoff");
        assert_eq!(status.state, "waiting");
        assert!(status.login_url.starts_with("https://"));
        let session_id = status.session_id.expect("Hub browser session id");
        cancel_sub2api_browser_login(session_id).expect("cancel browser session");
    }
}
