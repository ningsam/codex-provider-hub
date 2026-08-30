//! Active Codex route switching: native OpenAI, OAuth pool, pinned OAuth, or relay.
//!
//! The Hub keeps credentials in their existing stores. For Sub2API routes it only
//! changes the `schedulable` bit on already-authorized OpenAI accounts. Changes are
//! rolled back if a later account update or Codex config write fails.

use crate::gateway::{backup_file, codex_config_path, sub2api_dir};
use crate::http_util::{friendly_http_err, now_iso, BROWSER_UA, HTTP};
use reqwest::{blocking::Response, Method, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::fs;
use std::path::PathBuf;

const GATEWAY_BASE: &str = "http://127.0.0.1:18080";
const PAGE_SIZE: usize = 100;
const MAX_PAGES: usize = 20;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutingTarget {
    pub id: String,
    pub kind: String,
    pub account_id: Option<i64>,
    pub name: String,
    pub detail: String,
    pub available: bool,
    pub selected: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutingState {
    pub active_target: String,
    pub model_provider: String,
    pub targets: Vec<RoutingTarget>,
    pub gateway_error: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct RoutingPrefs {
    active_target: String,
    official_model: Option<String>,
    sub2api_model: Option<String>,
    sub2api_catalog: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RouteChoice {
    Official,
    Pool,
    OAuth(i64),
    Provider(i64),
}

#[derive(Debug, Clone)]
struct RouteAccount {
    id: i64,
    kind: String,
    name: String,
    email: String,
    status: String,
    error_message: String,
    schedulable: bool,
    base_url: String,
    model_mapping: Map<String, Value>,
}

impl RouteAccount {
    fn usable(&self) -> bool {
        let status = self.status.trim().to_ascii_lowercase();
        self.error_message.trim().is_empty()
            && matches!(status.as_str(), "active" | "ready" | "normal")
    }

    fn is_oauth(&self) -> bool {
        self.kind == "oauth"
    }

    fn is_provider(&self) -> bool {
        self.kind == "apikey"
    }
}

fn prefs_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_default()
        .join(".codex-provider-hub/routing-state.json")
}

fn load_prefs() -> RoutingPrefs {
    let path = prefs_path();
    fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn save_prefs(prefs: &RoutingPrefs) -> Result<(), String> {
    let path = prefs_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("create routing state directory: {e}"))?;
    }
    let out = serde_json::to_string_pretty(prefs)
        .map_err(|e| format!("serialize routing state: {e}"))?;
    fs::write(&path, out + "\n")
        .map_err(|e| format!("write {}: {e}", path.display()))
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
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| "admin login missing access_token".into())
}

fn response_value(resp: Response, context: &str) -> Result<Value, String> {
    let status = resp.status();
    let text = resp
        .text()
        .map_err(|e| format!("read {context} response: {e}"))?;
    let body: Value = serde_json::from_str(&text).unwrap_or_else(|_| json!({ "raw": text }));
    if !status.is_success() {
        let msg = body
            .get("message")
            .and_then(Value::as_str)
            .or_else(|| body.pointer("/error/message").and_then(Value::as_str))
            .or_else(|| body.get("raw").and_then(Value::as_str))
            .unwrap_or("request failed");
        return Err(format!("{context} HTTP {status}: {msg}"));
    }
    Ok(body)
}

fn admin_json(token: &str, method: Method, path: &str, body: Option<Value>) -> Result<Value, String> {
    let mut req = HTTP
        .request(method, format!("{GATEWAY_BASE}{path}"))
        .bearer_auth(token)
        .header("User-Agent", BROWSER_UA);
    if let Some(body) = body {
        req = req.json(&body);
    }
    let resp = req
        .send()
        .map_err(|e| friendly_http_err(&format!("admin {path}"), e))?;
    let parsed = response_value(resp, &format!("admin {path}"))?;
    if parsed.get("code").and_then(Value::as_i64) == Some(0) {
        Ok(parsed.get("data").cloned().unwrap_or(Value::Null))
    } else if parsed.get("data").is_some() {
        Ok(parsed["data"].clone())
    } else {
        Ok(parsed)
    }
}

fn parse_account(value: &Value) -> Option<RouteAccount> {
    let platform = value
        .get("platform")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_ascii_lowercase();
    if platform != "openai" {
        return None;
    }
    let kind = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_ascii_lowercase();
    if kind != "oauth" && kind != "apikey" {
        return None;
    }
    let id = value.get("id")?.as_i64()?;
    let credentials = value.get("credentials").cloned().unwrap_or_else(|| json!({}));
    let model_mapping = credentials
        .get("model_mapping")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    Some(RouteAccount {
        id,
        kind,
        name: value
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("unnamed")
            .to_string(),
        email: credentials
            .get("email")
            .and_then(Value::as_str)
            .or_else(|| value.get("email").and_then(Value::as_str))
            .unwrap_or("")
            .to_string(),
        status: value
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string(),
        error_message: value
            .get("error_message")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        schedulable: value
            .get("schedulable")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        base_url: credentials
            .get("base_url")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        model_mapping,
    })
}

fn list_accounts_with_token(token: &str) -> Result<Vec<RouteAccount>, String> {
    let mut out = Vec::new();
    for page in 1..=MAX_PAGES {
        let data = admin_json(
            token,
            Method::GET,
            &format!("/api/v1/admin/accounts?page={page}&page_size={PAGE_SIZE}"),
            None,
        )?;
        let items = data
            .get("items")
            .and_then(Value::as_array)
            .cloned()
            .or_else(|| data.as_array().cloned())
            .unwrap_or_default();
        let count = items.len();
        out.extend(items.iter().filter_map(parse_account));
        if count < PAGE_SIZE {
            break;
        }
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then(a.id.cmp(&b.id)));
    Ok(out)
}

fn list_accounts() -> Result<Vec<RouteAccount>, String> {
    let token = admin_login()?;
    list_accounts_with_token(&token)
}

fn set_schedulable_once(token: &str, account_id: i64, schedulable: bool) -> Result<(), String> {
    let path = format!("/api/v1/admin/accounts/{account_id}/schedulable");
    let resp = HTTP
        .post(format!("{GATEWAY_BASE}{path}"))
        .bearer_auth(token)
        .header("User-Agent", BROWSER_UA)
        .json(&json!({ "schedulable": schedulable }))
        .send()
        .map_err(|e| friendly_http_err(&format!("admin {path}"), e))?;
    let status = resp.status();
    if status.is_success() {
        return Ok(());
    }
    if status != StatusCode::NOT_FOUND && status != StatusCode::METHOD_NOT_ALLOWED {
        return response_value(resp, &format!("admin {path}")).map(|_| ());
    }

    // Compatibility fallback for older/newer Sub2API builds exposing only bulk-update.
    admin_json(
        token,
        Method::POST,
        "/api/v1/admin/accounts/bulk-update",
        Some(json!({
            "account_ids": [account_id],
            "schedulable": schedulable,
        })),
    )?;
    Ok(())
}

fn rollback_schedule(token: &str, changed: &[(i64, bool)]) {
    for (id, previous) in changed.iter().rev() {
        let _ = set_schedulable_once(token, *id, *previous);
    }
}

fn desired_schedulable(account: &RouteAccount, choice: &RouteChoice) -> bool {
    match choice {
        RouteChoice::Official => account.schedulable,
        RouteChoice::Pool => account.is_oauth() && account.usable(),
        RouteChoice::OAuth(id) => account.id == *id && account.is_oauth() && account.usable(),
        RouteChoice::Provider(id) => account.id == *id && account.is_provider() && account.usable(),
    }
}

fn apply_schedule(token: &str, accounts: &[RouteAccount], choice: &RouteChoice) -> Result<Vec<(i64, bool)>, String> {
    let mut changed = Vec::new();
    for account in accounts {
        let desired = desired_schedulable(account, choice);
        if account.schedulable == desired {
            continue;
        }
        if let Err(err) = set_schedulable_once(token, account.id, desired) {
            rollback_schedule(token, &changed);
            return Err(err);
        }
        changed.push((account.id, account.schedulable));
    }
    Ok(changed)
}

fn parse_choice(target_id: &str) -> Result<RouteChoice, String> {
    let target_id = target_id.trim();
    if target_id == "official" {
        return Ok(RouteChoice::Official);
    }
    if target_id == "pool" {
        return Ok(RouteChoice::Pool);
    }
    if let Some(id) = target_id.strip_prefix("oauth:") {
        return id
            .parse::<i64>()
            .map(RouteChoice::OAuth)
            .map_err(|_| "invalid OAuth route id".into());
    }
    if let Some(id) = target_id.strip_prefix("provider:") {
        return id
            .parse::<i64>()
            .map(RouteChoice::Provider)
            .map_err(|_| "invalid provider route id".into());
    }
    Err("unknown routing target".into())
}

fn choice_id(choice: &RouteChoice) -> String {
    match choice {
        RouteChoice::Official => "official".into(),
        RouteChoice::Pool => "pool".into(),
        RouteChoice::OAuth(id) => format!("oauth:{id}"),
        RouteChoice::Provider(id) => format!("provider:{id}"),
    }
}

fn validate_choice(choice: &RouteChoice, accounts: &[RouteAccount]) -> Result<(), String> {
    match choice {
        RouteChoice::Official => Ok(()),
        RouteChoice::Pool => {
            if accounts.iter().any(|a| a.is_oauth() && a.usable()) {
                Ok(())
            } else {
                Err("OAuth 号池没有可调度账号".into())
            }
        }
        RouteChoice::OAuth(id) => accounts
            .iter()
            .find(|a| a.id == *id && a.is_oauth())
            .ok_or_else(|| "未找到该 OAuth 账号".to_string())
            .and_then(|a| {
                if a.usable() {
                    Ok(())
                } else {
                    Err(format!("OAuth 账号 {} 当前不可用", a.name))
                }
            }),
        RouteChoice::Provider(id) => accounts
            .iter()
            .find(|a| a.id == *id && a.is_provider())
            .ok_or_else(|| "未找到该第三方中转".to_string())
            .and_then(|a| {
                if a.usable() {
                    Ok(())
                } else {
                    Err(format!("中转站 {} 当前不可用", a.name))
                }
            }),
    }
}

fn read_codex_config() -> Result<(PathBuf, String, toml::Value), String> {
    let path = codex_config_path();
    let raw = fs::read_to_string(&path)
        .map_err(|e| format!("read {}: {e}", path.display()))?;
    let doc: toml::Value = toml::from_str(&raw)
        .map_err(|e| format!("parse {}: {e}", path.display()))?;
    Ok((path, raw, doc))
}

fn doc_string(doc: &toml::Value, key: &str) -> Option<String> {
    doc.get(key).and_then(toml::Value::as_str).map(str::to_string)
}

fn model_provider(doc: &toml::Value) -> String {
    doc_string(doc, "model_provider").unwrap_or_else(|| "openai".into())
}

fn assignment_key(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    if trimmed.starts_with('#') || trimmed.starts_with('[') {
        return None;
    }
    trimmed.split_once('=').map(|(lhs, _)| lhs.trim())
}

fn toml_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| format!("\"{}\"", value.replace('"', "\\\"")))
}

/// Replace/remove a top-level TOML string key without touching provider tables or comments.
fn patch_top_level_string(raw: &str, key: &str, value: Option<&str>) -> String {
    let had_trailing_newline = raw.ends_with('\n');
    let mut out = Vec::<String>::new();
    let mut top = true;
    let mut handled = false;

    for line in raw.lines() {
        if top && line.trim_start().starts_with('[') {
            if !handled {
                if let Some(value) = value {
                    out.push(format!("{key} = {}", toml_string(value)));
                }
                handled = true;
            }
            top = false;
        }
        if top && assignment_key(line) == Some(key) {
            if let Some(value) = value {
                let indent = &line[..line.len() - line.trim_start().len()];
                out.push(format!("{indent}{key} = {}", toml_string(value)));
            }
            handled = true;
            continue;
        }
        out.push(line.to_string());
    }

    if !handled {
        if let Some(value) = value {
            out.push(format!("{key} = {}", toml_string(value)));
        }
    }

    let mut joined = out.join("\n");
    if had_trailing_newline {
        joined.push('\n');
    }
    joined
}

fn write_codex_config(path: &PathBuf, content: &str) -> Result<(), String> {
    let _ = backup_file(path)?;
    fs::write(path, content).map_err(|e| format!("write {}: {e}", path.display()))
}

fn default_catalog_path() -> Option<String> {
    let path = dirs::home_dir()
        .unwrap_or_default()
        .join(".codex/model-catalogs/aihub-sub2api.json");
    path.is_file().then(|| path.display().to_string())
}

fn resolve_raw_model(accounts: &[RouteAccount], model: &str) -> Option<String> {
    for account in accounts {
        if let Some(raw) = account.model_mapping.get(model).and_then(Value::as_str) {
            if !raw.trim().is_empty() {
                return Some(raw.to_string());
            }
        }
    }
    let lower = model.to_ascii_lowercase();
    if lower.starts_with("gpt-")
        || lower.starts_with("o1")
        || lower.starts_with("o3")
        || lower.starts_with("codex")
    {
        return Some(model.to_string());
    }
    None
}

fn mapping_for_choice<'a>(choice: &RouteChoice, accounts: &'a [RouteAccount]) -> Vec<&'a Map<String, Value>> {
    match choice {
        RouteChoice::Pool => accounts
            .iter()
            .filter(|a| a.is_oauth() && a.usable())
            .map(|a| &a.model_mapping)
            .collect(),
        RouteChoice::OAuth(id) => accounts
            .iter()
            .filter(|a| a.id == *id && a.is_oauth())
            .map(|a| &a.model_mapping)
            .collect(),
        RouteChoice::Provider(id) => accounts
            .iter()
            .filter(|a| a.id == *id && a.is_provider())
            .map(|a| &a.model_mapping)
            .collect(),
        RouteChoice::Official => Vec::new(),
    }
}

fn mapping_has_key(mappings: &[&Map<String, Value>], key: &str) -> bool {
    mappings.iter().any(|m| m.contains_key(key))
}

fn key_for_raw(mappings: &[&Map<String, Value>], raw: &str) -> Option<String> {
    let mut keys = Vec::new();
    for mapping in mappings {
        for (key, value) in *mapping {
            if value.as_str() == Some(raw) {
                keys.push(key.clone());
            }
        }
    }
    keys.sort();
    keys.into_iter().next()
}

fn first_mapping_key(mappings: &[&Map<String, Value>]) -> Option<String> {
    let mut keys: Vec<String> = mappings
        .iter()
        .flat_map(|m| m.keys().cloned())
        .collect();
    keys.sort();
    keys.dedup();
    keys.into_iter().next()
}

fn choose_sub2api_model(
    choice: &RouteChoice,
    accounts: &[RouteAccount],
    current_model: Option<&str>,
    preferred_model: Option<&str>,
) -> Option<String> {
    let mappings = mapping_for_choice(choice, accounts);
    for preferred in [preferred_model, current_model].into_iter().flatten() {
        if mapping_has_key(&mappings, preferred) {
            return Some(preferred.to_string());
        }
    }

    let raw = current_model
        .and_then(|model| resolve_raw_model(accounts, model))
        .or_else(|| preferred_model.and_then(|model| resolve_raw_model(accounts, model)));
    if let Some(raw) = raw {
        if let Some(key) = key_for_raw(&mappings, &raw) {
            return Some(key);
        }
        if mappings.is_empty() {
            return Some(raw);
        }
    }

    first_mapping_key(&mappings)
        .or_else(|| preferred_model.map(str::to_string))
        .or_else(|| current_model.map(str::to_string))
}

fn safe_host(base_url: &str) -> String {
    let stripped = base_url
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    stripped
        .split('/')
        .next()
        .unwrap_or(stripped)
        .rsplit_once('@')
        .map(|(_, host)| host)
        .unwrap_or(stripped)
        .to_string()
}

fn target_matches(choice: &RouteChoice, accounts: &[RouteAccount]) -> bool {
    let scheduled: Vec<&RouteAccount> = accounts.iter().filter(|a| a.schedulable).collect();
    match choice {
        RouteChoice::Official => false,
        RouteChoice::Pool => {
            scheduled.iter().any(|a| a.is_oauth()) && scheduled.iter().all(|a| a.is_oauth())
        }
        RouteChoice::OAuth(id) => {
            scheduled.len() == 1 && scheduled[0].is_oauth() && scheduled[0].id == *id
        }
        RouteChoice::Provider(id) => {
            scheduled.len() == 1 && scheduled[0].is_provider() && scheduled[0].id == *id
        }
    }
}

fn infer_active_target(provider: &str, accounts: &[RouteAccount], prefs: &RoutingPrefs) -> String {
    if provider == "openai" {
        return "official".into();
    }
    if provider != "sub2api" {
        return "unmanaged".into();
    }

    if !prefs.active_target.is_empty() {
        if let Ok(choice) = parse_choice(&prefs.active_target) {
            if choice != RouteChoice::Official && target_matches(&choice, accounts) {
                return prefs.active_target.clone();
            }
        }
    }

    let scheduled: Vec<&RouteAccount> = accounts.iter().filter(|a| a.schedulable).collect();
    if scheduled.len() == 1 {
        let account = scheduled[0];
        if account.is_oauth() {
            return format!("oauth:{}", account.id);
        }
        if account.is_provider() {
            return format!("provider:{}", account.id);
        }
    }
    if !scheduled.is_empty() && scheduled.iter().all(|a| a.is_oauth()) {
        return "pool".into();
    }
    "unmanaged".into()
}

fn build_targets(accounts: &[RouteAccount], active: &str) -> Vec<RoutingTarget> {
    let available_oauth = accounts.iter().filter(|a| a.is_oauth() && a.usable()).count();
    let mut targets = vec![
        RoutingTarget {
            id: "official".into(),
            kind: "official".into(),
            account_id: None,
            name: "OpenAI Official".into(),
            detail: "Current Codex login".into(),
            available: true,
            selected: active == "official",
        },
        RoutingTarget {
            id: "pool".into(),
            kind: "pool".into(),
            account_id: None,
            name: "Sub2API OAuth Pool".into(),
            detail: format!("{available_oauth} available OAuth account(s)"),
            available: available_oauth > 0,
            selected: active == "pool",
        },
    ];

    for account in accounts.iter().filter(|a| a.is_oauth()) {
        targets.push(RoutingTarget {
            id: format!("oauth:{}", account.id),
            kind: "oauth".into(),
            account_id: Some(account.id),
            name: account.name.clone(),
            detail: if account.email.is_empty() {
                account.status.clone()
            } else {
                account.email.clone()
            },
            available: account.usable(),
            selected: active == format!("oauth:{}", account.id),
        });
    }

    for account in accounts.iter().filter(|a| a.is_provider()) {
        let host = safe_host(&account.base_url);
        targets.push(RoutingTarget {
            id: format!("provider:{}", account.id),
            kind: "provider".into(),
            account_id: Some(account.id),
            name: account.name.clone(),
            detail: if host.is_empty() { account.status.clone() } else { host },
            available: account.usable(),
            selected: active == format!("provider:{}", account.id),
        });
    }
    targets
}

fn build_state() -> Result<RoutingState, String> {
    let (_, _, doc) = read_codex_config()?;
    let provider = model_provider(&doc);
    let prefs = load_prefs();
    let (accounts, gateway_error) = match list_accounts() {
        Ok(accounts) => (accounts, None),
        Err(err) => (Vec::new(), Some(err)),
    };
    let active = infer_active_target(&provider, &accounts, &prefs);
    let targets = build_targets(&accounts, &active);
    Ok(RoutingState {
        active_target: active,
        model_provider: provider,
        targets,
        gateway_error,
        updated_at: now_iso(),
    })
}

#[tauri::command]
pub fn get_routing_state() -> Result<RoutingState, String> {
    build_state()
}

#[tauri::command]
pub fn switch_routing_target(target_id: String) -> Result<RoutingState, String> {
    let choice = parse_choice(&target_id)?;
    let (cfg_path, raw, doc) = read_codex_config()?;
    let current_provider = model_provider(&doc);
    let current_model = doc_string(&doc, "model");
    let current_catalog = doc_string(&doc, "model_catalog_json");
    let mut prefs = load_prefs();

    if choice == RouteChoice::Official {
        // Native official mode must remain selectable even while Sub2API is down.
        if current_provider == "sub2api" {
            if current_model.is_some() {
                prefs.sub2api_model = current_model.clone();
            }
            if current_catalog.is_some() {
                prefs.sub2api_catalog = current_catalog.clone();
            }
        } else if current_provider == "openai" && current_model.is_some() {
            prefs.official_model = current_model.clone();
        }

        // Best effort: translate a prefixed Sub2API model back to its raw OpenAI id.
        let translated_model = if prefs.official_model.is_some() {
            prefs.official_model.clone()
        } else if let Some(model) = current_model.as_deref() {
            list_accounts()
                .ok()
                .and_then(|accounts| resolve_raw_model(&accounts, model))
        } else {
            None
        };

        let mut patched = patch_top_level_string(&raw, "model_provider", Some("openai"));
        patched = patch_top_level_string(&patched, "model_catalog_json", None);
        patched = patch_top_level_string(&patched, "model", translated_model.as_deref());
        write_codex_config(&cfg_path, &patched)?;

        prefs.active_target = "official".into();
        if translated_model.is_some() {
            prefs.official_model = translated_model;
        }
        let _ = save_prefs(&prefs);
        crate::http_util::invalidate_cache("gateway_status");
        crate::http_util::invalidate_cache("sub2api_usage");
        return build_state();
    }

    // Sub2API modes require the local gateway and an existing sub2api provider table.
    if doc
        .get("model_providers")
        .and_then(|v| v.get("sub2api"))
        .is_none()
    {
        return Err("config.toml missing [model_providers.sub2api]".into());
    }

    let token = admin_login()?;
    let accounts = list_accounts_with_token(&token)?;
    validate_choice(&choice, &accounts)?;

    if current_provider == "openai" && current_model.is_some() {
        prefs.official_model = current_model.clone();
    }
    if current_provider == "sub2api" {
        if current_model.is_some() {
            prefs.sub2api_model = current_model.clone();
        }
        if current_catalog.is_some() {
            prefs.sub2api_catalog = current_catalog.clone();
        }
    }

    let next_catalog = prefs
        .sub2api_catalog
        .clone()
        .or_else(|| current_catalog.clone())
        .or_else(default_catalog_path);
    let next_model = choose_sub2api_model(
        &choice,
        &accounts,
        current_model.as_deref(),
        prefs.sub2api_model.as_deref(),
    );

    let changed = apply_schedule(&token, &accounts, &choice)?;

    let mut patched = patch_top_level_string(&raw, "model_provider", Some("sub2api"));
    if let Some(catalog) = next_catalog.as_deref() {
        patched = patch_top_level_string(&patched, "model_catalog_json", Some(catalog));
    }
    if let Some(model) = next_model.as_deref() {
        patched = patch_top_level_string(&patched, "model", Some(model));
    }

    if let Err(err) = write_codex_config(&cfg_path, &patched) {
        rollback_schedule(&token, &changed);
        return Err(err);
    }

    prefs.active_target = choice_id(&choice);
    if next_catalog.is_some() {
        prefs.sub2api_catalog = next_catalog;
    }
    if next_model.is_some() {
        prefs.sub2api_model = next_model;
    }
    let _ = save_prefs(&prefs);

    crate::http_util::invalidate_cache("gateway_status");
    crate::http_util::invalidate_cache("sub2api_usage");
    build_state()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patches_only_top_level_toml_keys() {
        let raw = "model = \"old\"\nmodel_provider = \"sub2api\"\n\n[model_providers.sub2api]\nmodel = \"leave-me\"\n";
        let out = patch_top_level_string(raw, "model_provider", Some("openai"));
        let out = patch_top_level_string(&out, "model", None);
        assert!(!out.lines().any(|line| line == "model = \"old\""));
        assert!(out.contains("model_provider = \"openai\""));
        assert!(out.contains("[model_providers.sub2api]\nmodel = \"leave-me\""));
    }

    #[test]
    fn parses_route_ids() {
        assert_eq!(parse_choice("official").unwrap(), RouteChoice::Official);
        assert_eq!(parse_choice("pool").unwrap(), RouteChoice::Pool);
        assert_eq!(parse_choice("oauth:12").unwrap(), RouteChoice::OAuth(12));
        assert_eq!(parse_choice("provider:9").unwrap(), RouteChoice::Provider(9));
        assert!(parse_choice("relay:9").is_err());
    }
}
