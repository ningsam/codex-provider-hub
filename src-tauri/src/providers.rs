//! Custom OpenAI-compatible apikey providers → Sub2API accounts + Codex catalog.

use crate::gateway::{backup_file, catalog_path_from_config, codex_config_path, sub2api_dir};
use crate::http_util::{friendly_http_err, BROWSER_UA, HTTP};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::fs;
use std::time::Duration;

const GATEWAY_BASE: &str = "http://127.0.0.1:18080";
const OPENAI_DEFAULT_GROUP_ID: i64 = 2;
const PROBE_TIMEOUT: Duration = Duration::from_secs(30);

/// Safe provider row for the UI (never includes a raw API key).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderInfo {
    pub id: i64,
    pub name: String,
    pub base_url: String,
    pub base_url_masked: String,
    pub prefix: String,
    pub status: String,
    pub model_count: u32,
    pub has_api_key: bool,
    pub schedulable: bool,
    pub error_message: String,
}

/// Result of adding / syncing a provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderMutationResult {
    pub provider: ProviderInfo,
    pub models_synced: u32,
    pub model_ids: Vec<String>,
    pub allowlist_updated: bool,
    pub restart_required: bool,
    pub hint: Option<String>,
}

fn probe_http() -> Client {
    Client::builder()
        .timeout(PROBE_TIMEOUT)
        .user_agent(BROWSER_UA)
        .build()
        .expect("build probe client")
}

fn admin_json(method: reqwest::Method, path: &str, body: Option<Value>) -> Result<Value, String> {
    let url = if path.starts_with("http") {
        path.to_string()
    } else {
        format!("{GATEWAY_BASE}{path}")
    };
    for attempt in 0..=1 {
        let token = crate::sub2api::admin_login()?;
        let mut req = HTTP
            .request(method.clone(), &url)
            .bearer_auth(&token)
            .header("User-Agent", BROWSER_UA);
        if let Some(body) = body.as_ref() {
            req = req.json(body);
        }
        let resp = req
            .send()
            .map_err(|e| friendly_http_err(&format!("admin {path}"), e))?;
        let status = resp.status();
        let text = resp
            .text()
            .map_err(|e| format!("read admin {path} body: {e}"))?;
        if status.as_u16() == 401 {
            crate::sub2api::invalidate_admin_token(&token);
            if attempt == 0 {
                continue;
            }
        }
        let parsed: Value = serde_json::from_str(&text).unwrap_or_else(|_| json!({ "raw": text }));
        if !status.is_success() {
            let raw_message = parsed
                .get("message")
                .and_then(Value::as_str)
                .or_else(|| parsed.pointer("/error/message").and_then(Value::as_str))
                .unwrap_or(text.trim());
            let message = crate::sub2api::safe_reason(raw_message, "request failed");
            return Err(format!("admin {path} HTTP {status}: {message}"));
        }
        if parsed.get("code").and_then(Value::as_i64) == Some(0) {
            return Ok(parsed.get("data").cloned().unwrap_or(Value::Null));
        }
        if parsed.get("data").is_some() {
            return Ok(parsed["data"].clone());
        }
        return Ok(parsed);
    }
    unreachable!("provider admin retry loop always returns")
}

fn split_url(url: &str) -> Result<(String, String, String), String> {
    let (scheme, rest) = if let Some(r) = url.strip_prefix("https://") {
        ("https", r)
    } else if let Some(r) = url.strip_prefix("http://") {
        ("http", r)
    } else {
        return Err("Base URL 仅支持 http/https".into());
    };
    let rest = rest.split('#').next().unwrap_or(rest);
    let rest = rest.split('?').next().unwrap_or(rest);
    // Strip userinfo if present.
    let rest = rest
        .rsplit_once('@')
        .map(|(_, hostpath)| hostpath)
        .unwrap_or(rest);
    let (host, path) = match rest.split_once('/') {
        Some((h, p)) => (h.to_string(), format!("/{p}")),
        None => (rest.to_string(), String::new()),
    };
    if host.is_empty() {
        return Err("Base URL 缺少主机名".into());
    }
    Ok((scheme.into(), host, path))
}

fn mask_base_url(url: &str) -> String {
    match split_url(url) {
        Ok((scheme, host, _)) => format!("{scheme}://{host}/•••"),
        Err(_) => {
            if url.len() <= 12 {
                "•••".into()
            } else {
                format!("{}…•••", &url[..12])
            }
        }
    }
}

pub(crate) fn normalize_base_url(raw: &str) -> Result<String, String> {
    let mut s = raw.trim().to_string();
    if s.is_empty() {
        return Err("Base URL 不能为空".into());
    }
    if !s.contains("://") {
        s = format!("https://{s}");
    }
    let (scheme, host, path) = split_url(&s)?;
    let mut path = path.trim_end_matches('/').to_string();
    if path.is_empty() {
        path = "/v1".into();
    } else if !path.ends_with("/v1") {
        path = format!("{path}/v1");
    }
    Ok(format!("{scheme}://{host}{path}"))
}

pub(crate) fn slugify_prefix(name: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for ch in name.trim().chars() {
        let c = ch.to_ascii_lowercase();
        if c.is_ascii_alphanumeric() {
            out.push(c);
            prev_dash = false;
        } else if !prev_dash && !out.is_empty() {
            out.push('-');
            prev_dash = true;
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        "provider".into()
    } else {
        out.chars().take(32).collect()
    }
}

fn validate_prefix(prefix: &str) -> Result<String, String> {
    let p = prefix.trim().trim_matches('-').to_ascii_lowercase();
    if p.is_empty() {
        return Err("模型前缀不能为空".into());
    }
    if !p
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err("模型前缀仅允许小写字母、数字与连字符".into());
    }
    if p.len() > 32 {
        return Err("模型前缀过长（最多 32）".into());
    }
    Ok(p)
}

fn infer_prefix_from_mapping(mapping: &Map<String, Value>) -> String {
    for (k, v) in mapping {
        if let Some(raw) = v.as_str() {
            let expected_suffix = format!("-{raw}");
            if let Some(prefix) = k.strip_suffix(&expected_suffix) {
                if !prefix.is_empty() {
                    return prefix.to_string();
                }
            }
        }
    }
    String::new()
}

fn host_from_base_url(base_url: &str) -> Option<String> {
    split_url(base_url).ok().map(|(_, host, _)| host)
}

fn append_hint(hint: &mut Option<String>, message: String) {
    match hint {
        Some(existing) if !existing.is_empty() => {
            existing.push(' ');
            existing.push_str(&message);
        }
        Some(existing) => *existing = message,
        None => *hint = Some(message),
    }
}

fn build_model_mapping(prefix: &str, model_ids: &[String]) -> Map<String, Value> {
    let mut map = Map::new();
    for raw in model_ids {
        let raw = raw.trim();
        if raw.is_empty() {
            continue;
        }
        let key = format!("{prefix}-{raw}");
        map.insert(key, Value::String(raw.to_string()));
    }
    map
}

fn extract_model_ids(body: &Value) -> Vec<String> {
    let mut ids = Vec::new();
    if let Some(arr) = body.get("data").and_then(|d| d.as_array()) {
        for item in arr {
            if let Some(id) = item.get("id").and_then(|v| v.as_str()) {
                if !id.is_empty() {
                    ids.push(id.to_string());
                }
            }
        }
    } else if let Some(arr) = body.get("models").and_then(|d| d.as_array()) {
        for item in arr {
            if let Some(id) = item.as_str() {
                ids.push(id.to_string());
            } else if let Some(id) = item.get("id").and_then(|v| v.as_str()) {
                ids.push(id.to_string());
            }
        }
    } else if let Some(arr) = body.as_array() {
        for item in arr {
            if let Some(id) = item.as_str() {
                ids.push(id.to_string());
            } else if let Some(id) = item.get("id").and_then(|v| v.as_str()) {
                ids.push(id.to_string());
            }
        }
    }
    ids.sort();
    ids.dedup();
    ids
}

/// Probe `{base_url}/models` with the given API key (never logged).
pub(crate) fn probe_upstream_models(base_url: &str, api_key: &str) -> Result<Vec<String>, String> {
    let key = api_key.trim();
    if key.is_empty() {
        return Err("API Key 不能为空".into());
    }
    let models_url = format!("{}/models", base_url.trim_end_matches('/'));
    let resp = probe_http()
        .get(&models_url)
        .bearer_auth(key)
        .header("User-Agent", BROWSER_UA)
        .send()
        .map_err(|e| friendly_http_err("上游 /models", e))?;
    let status = resp.status();
    let text = resp.text().map_err(|e| format!("读取上游 /models: {e}"))?;
    if !status.is_success() {
        let snippet: String = text.chars().take(180).collect();
        return Err(format!(
            "探测上游模型失败 HTTP {status}（请检查 Base URL / API Key）。{snippet}"
        ));
    }
    let body: Value =
        serde_json::from_str(&text).map_err(|e| format!("解析上游 /models JSON: {e}"))?;
    let ids = extract_model_ids(&body);
    if ids.is_empty() {
        return Err("上游 /models 未返回任何模型 id".into());
    }
    Ok(ids)
}

fn sync_upstream_via_admin(account_id: i64) -> Result<Vec<String>, String> {
    let data = admin_json(
        reqwest::Method::POST,
        &format!("/api/v1/admin/accounts/{account_id}/models/sync-upstream"),
        None,
    )?;
    let ids = extract_model_ids(&data);
    if ids.is_empty() {
        return Err("Sub2API sync-upstream 未返回模型".into());
    }
    Ok(ids)
}

fn account_to_info(acc: &Value) -> Option<ProviderInfo> {
    let id = acc.get("id")?.as_i64()?;
    let name = acc
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let status = acc
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();
    let schedulable = acc
        .get("schedulable")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let error_message = acc
        .get("error_message")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let creds = acc.get("credentials").cloned().unwrap_or(json!({}));
    let base_url = creds
        .get("base_url")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let mapping = creds
        .get("model_mapping")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();
    let prefix = infer_prefix_from_mapping(&mapping);
    let has_api_key = acc
        .pointer("/credentials_status/has_api_key")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    Some(ProviderInfo {
        id,
        name,
        base_url_masked: mask_base_url(&base_url),
        base_url,
        prefix,
        status,
        model_count: mapping.len() as u32,
        has_api_key,
        schedulable,
        error_message,
    })
}

fn list_apikey_accounts() -> Result<Vec<Value>, String> {
    let mut page = 1i64;
    let mut all = Vec::new();
    loop {
        let data = admin_json(
            reqwest::Method::GET,
            &format!("/api/v1/admin/accounts?page={page}&page_size=50&type=apikey"),
            None,
        )?;
        let items = data
            .get("items")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let total_pages = data.get("pages").and_then(|v| v.as_i64()).unwrap_or(1);
        for item in items {
            if item.get("type").and_then(|t| t.as_str()) == Some("apikey") {
                all.push(item);
            }
        }
        if page >= total_pages {
            break;
        }
        page += 1;
    }
    Ok(all)
}

fn get_account(id: i64) -> Result<Value, String> {
    admin_json(
        reqwest::Method::GET,
        &format!("/api/v1/admin/accounts/{id}"),
        None,
    )
}

fn catalog_path() -> Result<std::path::PathBuf, String> {
    let cfg_path = codex_config_path();
    let raw =
        fs::read_to_string(&cfg_path).map_err(|e| format!("read {}: {e}", cfg_path.display()))?;
    let doc: toml::Value = toml::from_str(&raw).map_err(|e| format!("parse config.toml: {e}"))?;
    // Refuse to proceed if provider table missing — never invent a new id.
    if doc
        .get("model_providers")
        .and_then(|v| v.get("sub2api"))
        .is_none()
    {
        return Err(
            "config.toml missing [model_providers.sub2api] — refusing to touch catalog".into(),
        );
    }
    Ok(catalog_path_from_config(&doc))
}

fn load_catalog() -> Result<(std::path::PathBuf, Value), String> {
    let path = catalog_path()?;
    let raw =
        fs::read_to_string(&path).map_err(|e| format!("read catalog {}: {e}", path.display()))?;
    let catalog: Value = serde_json::from_str(&raw).map_err(|e| format!("parse catalog: {e}"))?;
    Ok((path, catalog))
}

fn template_model_entry(catalog: &Value) -> Value {
    let models = catalog
        .get("models")
        .and_then(|m| m.as_array())
        .cloned()
        .unwrap_or_default();
    let template = models
        .iter()
        .find(|m| {
            m.get("slug")
                .and_then(|s| s.as_str())
                .map(|s| s.starts_with("aihub-"))
                .unwrap_or(false)
        })
        .or_else(|| models.first())
        .cloned()
        .unwrap_or_else(|| {
            json!({
                "slug": "template",
                "display_name": "Template",
                "description": "",
                "default_reasoning_level": "low",
                "supported_reasoning_levels": [
                    {"effort": "low", "description": "Fast responses with lighter reasoning"},
                    {"effort": "medium", "description": "Balances speed and reasoning depth for everyday tasks"},
                    {"effort": "high", "description": "Greater reasoning depth for complex problems"},
                    {"effort": "xhigh", "description": "Extra high reasoning depth for complex problems"},
                    {"effort": "max", "description": "Maximum reasoning depth for the hardest problems"},
                    {"effort": "ultra", "description": "Maximum reasoning with automatic task delegation"}
                ],
                "shell_type": "shell_command",
                "visibility": "list",
                "supported_in_api": true,
                "priority": 1,
                "prefer_websockets": false,
                "context_window": 272000,
                "max_context_window": 272000,
                "input_modalities": ["text", "image"],
                "supports_parallel_tool_calls": true
            })
        });
    template
}

fn make_catalog_entry(
    template: &Value,
    display_name: &str,
    prefix: &str,
    raw_model: &str,
) -> Value {
    let mut entry = template.clone();
    let slug = format!("{prefix}-{raw_model}");
    if let Some(obj) = entry.as_object_mut() {
        obj.insert("slug".into(), json!(slug));
        obj.insert(
            "display_name".into(),
            json!(format!("{display_name} | {raw_model}")),
        );
        obj.insert(
            "description".into(),
            json!(format!("{display_name}: Upstream model {raw_model}.")),
        );
        obj.insert("visibility".into(), json!("list"));
        obj.insert("supported_in_api".into(), json!(true));
        obj.insert("prefer_websockets".into(), json!(false));
        // Keep reasoning / tool fields from the AIHub template.
    }
    entry
}

fn upsert_catalog_models(
    display_name: &str,
    prefix: &str,
    model_ids: &[String],
) -> Result<u32, String> {
    let (path, mut catalog) = load_catalog()?;
    let _bak = backup_file(&path)?;
    let template = template_model_entry(&catalog);
    let prefix_dash = format!("{prefix}-");
    let existing = catalog
        .get_mut("models")
        .and_then(|m| m.as_array_mut())
        .ok_or_else(|| "catalog missing models[]".to_string())?;

    existing.retain(|m| {
        m.get("slug")
            .and_then(|s| s.as_str())
            .map(|s| !s.starts_with(&prefix_dash))
            .unwrap_or(true)
    });

    let mut added = 0u32;
    for raw in model_ids {
        existing.push(make_catalog_entry(&template, display_name, prefix, raw));
        added += 1;
    }

    let out =
        serde_json::to_string_pretty(&catalog).map_err(|e| format!("serialize catalog: {e}"))?;
    fs::write(&path, out + "\n").map_err(|e| format!("write catalog: {e}"))?;
    crate::http_util::invalidate_cache("gateway_status");
    Ok(added)
}

fn remove_catalog_prefix(prefix: &str) -> Result<u32, String> {
    let (path, mut catalog) = load_catalog()?;
    let _bak = backup_file(&path)?;
    let prefix_dash = format!("{prefix}-");
    let models = catalog
        .get_mut("models")
        .and_then(|m| m.as_array_mut())
        .ok_or_else(|| "catalog missing models[]".to_string())?;
    let before = models.len();
    models.retain(|m| {
        m.get("slug")
            .and_then(|s| s.as_str())
            .map(|s| !s.starts_with(&prefix_dash))
            .unwrap_or(true)
    });
    let removed = (before - models.len()) as u32;
    let out =
        serde_json::to_string_pretty(&catalog).map_err(|e| format!("serialize catalog: {e}"))?;
    fs::write(&path, out + "\n").map_err(|e| format!("write catalog: {e}"))?;
    crate::http_util::invalidate_cache("gateway_status");
    Ok(removed)
}

/// Try to append host to Sub2API compose allowlist. No admin API exists for this.
/// When `SECURITY_URL_ALLOWLIST_ENABLED` is false (current local default), skip edits.
fn try_add_upstream_host_allowlist(host: &str) -> Result<(bool, bool, Option<String>), String> {
    if host.is_empty() {
        return Ok((false, false, None));
    }
    let compose = sub2api_dir().join("compose.yaml");
    if !compose.is_file() {
        return Ok((
            false,
            false,
            Some(format!(
                "未找到 compose.yaml；若出现 502 host not allowed，请在 Sub2API 放行域名 {host}"
            )),
        ));
    }
    let raw =
        fs::read_to_string(&compose).map_err(|e| format!("read {}: {e}", compose.display()))?;
    // Local Hub default: allowlist disabled so any https upstream works without restart.
    if raw.lines().any(|l| {
        let t = l.trim();
        t.starts_with("SECURITY_URL_ALLOWLIST_ENABLED:") && t.contains("false")
    }) {
        return Ok((
            false,
            false,
            Some(format!(
                "当前 Sub2API 已关闭 URL 白名单，可直接使用 {host}（无需改 compose / 重启）"
            )),
        ));
    }
    let marker = "SECURITY_URL_ALLOWLIST_UPSTREAM_HOSTS:";
    let Some(line) = raw.lines().find(|l| l.contains(marker)) else {
        return Ok((
            false,
            false,
            Some(format!(
                "compose.yaml 无 UpstreamHosts；若 502 host not allowed，需放行 {host}"
            )),
        ));
    };
    // Extract quoted value.
    let value_part = line
        .split_once(':')
        .map(|(_, v)| v.trim().trim_matches('"'))
        .unwrap_or("");
    let mut hosts: Vec<String> = value_part
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let already = hosts.iter().any(|h| {
        h == host
            || (h.starts_with("*.") && host.ends_with(&h[1..]))
            || (host.starts_with("*.") == false && h == host)
    });
    if already {
        return Ok((
            false,
            false,
            Some(format!("域名 {host} 已在 Sub2API UpstreamHosts 白名单中")),
        ));
    }
    hosts.push(host.to_string());
    let new_value = hosts.join(",");
    let mut new_raw = String::new();
    for l in raw.lines() {
        if l.contains(marker) {
            // Preserve indentation / quoting style.
            let indent = &l[..l.len() - l.trim_start().len()];
            new_raw.push_str(indent);
            new_raw.push_str(marker);
            new_raw.push(' ');
            new_raw.push('"');
            new_raw.push_str(&new_value);
            new_raw.push('"');
            new_raw.push('\n');
        } else {
            new_raw.push_str(l);
            new_raw.push('\n');
        }
    }
    let _ = backup_file(&compose);
    fs::write(&compose, new_raw).map_err(|e| format!("write compose.yaml: {e}"))?;
    Ok((
        true,
        true,
        Some(format!(
            "已将 {host} 写入 compose.yaml UpstreamHosts；需重启 Sub2API（./sub2api down && ./sub2api up）后生效。若仍 502 host not allowed，请确认白名单。"
        )),
    ))
}

fn existing_prefixes() -> Result<HashMap<String, i64>, String> {
    let mut map = HashMap::new();
    for acc in list_apikey_accounts()? {
        if let Some(info) = account_to_info(&acc) {
            if !info.prefix.is_empty() {
                map.insert(info.prefix, info.id);
            }
        }
    }
    Ok(map)
}

fn create_apikey_account(
    name: &str,
    base_url: &str,
    api_key: &str,
    mapping: &Map<String, Value>,
) -> Result<Value, String> {
    let payload = json!({
        "name": name,
        "platform": "openai",
        "type": "apikey",
        "concurrency": 3,
        "priority": 50,
        "rate_multiplier": 1.0,
        "group_ids": [OPENAI_DEFAULT_GROUP_ID],
        "auto_pause_on_expired": true,
        "credentials": {
            "base_url": base_url,
            "api_key": api_key,
            "model_mapping": mapping,
        },
        "extra": {
            "openai_long_context_billing_enabled": false,
            "openai_responses_mode": "force_responses",
            "openai_responses_supported": false,
        }
    });
    admin_json(
        reqwest::Method::POST,
        "/api/v1/admin/accounts",
        Some(payload),
    )
}

fn update_account_mapping(
    account_id: i64,
    base_url: &str,
    mapping: &Map<String, Value>,
) -> Result<Value, String> {
    // api_key omitted on purpose — Sub2API merges preserving sensitive creds.
    let payload = json!({
        "credentials": {
            "base_url": base_url,
            "model_mapping": mapping,
        }
    });
    admin_json(
        reqwest::Method::PUT,
        &format!("/api/v1/admin/accounts/{account_id}"),
        Some(payload),
    )
}

/// List apikey-type Sub2API accounts (safe fields only).
#[tauri::command]
pub fn list_providers() -> Result<Vec<ProviderInfo>, String> {
    let mut out: Vec<ProviderInfo> = list_apikey_accounts()?
        .iter()
        .filter_map(account_to_info)
        .collect();
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(out)
}

/// Add an OpenAI-compatible apikey provider: probe models → Sub2API account → Codex catalog.
#[tauri::command]
pub fn add_provider(
    name: String,
    base_url: String,
    api_key: String,
    prefix: Option<String>,
    probe_models: Option<bool>,
) -> Result<ProviderMutationResult, String> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("显示名不能为空".into());
    }
    let base_url = normalize_base_url(&base_url)?;
    let api_key = api_key.trim().to_string();
    if api_key.is_empty() {
        return Err("API Key 不能为空".into());
    }
    // Never log api_key.
    let prefix = match prefix {
        Some(p) if !p.trim().is_empty() => validate_prefix(&p)?,
        _ => validate_prefix(&slugify_prefix(&name))?,
    };
    let do_probe = probe_models.unwrap_or(true);

    let prefixes = existing_prefixes()?;
    if let Some(existing_id) = prefixes.get(&prefix) {
        return Err(format!(
            "前缀 `{prefix}` 已被账号 #{existing_id} 占用，请换一个前缀"
        ));
    }

    let model_ids = if do_probe {
        probe_upstream_models(&base_url, &api_key)?
    } else {
        Vec::new()
    };
    let mapping = build_model_mapping(&prefix, &model_ids);

    let created = create_apikey_account(&name, &base_url, &api_key, &mapping)?;
    let created_id = created
        .get("id")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| "创建账号响应缺少 id".to_string())?;
    // Re-fetch: create response credentials are redacted the same way as GET.
    let fetched = get_account(created_id).unwrap_or(created);
    let provider = account_to_info(&fetched).ok_or_else(|| "创建成功但无法解析账号".to_string())?;

    let mut models_synced = 0u32;
    if !model_ids.is_empty() {
        models_synced = upsert_catalog_models(&name, &prefix, &model_ids)?;
    }

    let host = host_from_base_url(&base_url).unwrap_or_default();
    let (allowlist_updated, restart_required, hint) = try_add_upstream_host_allowlist(&host)?;
    let mut hint = hint;
    if hint.is_none() {
        hint = Some(format!(
            "若请求返回 502 host not allowed，需在 Sub2API 放行域名 {host}"
        ));
    }
    if let Err(error) = crate::sub2api::refresh_managed_route_after_pool_change() {
        append_hint(
            &mut hint,
            format!(
                "路由组同步失败：{}",
                crate::sub2api::safe_reason(&error, "未知错误")
            ),
        );
    }
    crate::http_util::invalidate_cache("sub2api_usage");

    // Drop api_key from scope intentionally (no further use / no logging).
    drop(api_key);

    Ok(ProviderMutationResult {
        provider: ProviderInfo {
            prefix,
            model_count: model_ids.len() as u32,
            ..provider
        },
        models_synced,
        model_ids,
        allowlist_updated,
        restart_required,
        hint,
    })
}

/// Remove a Sub2API apikey account and matching catalog models for its prefix.
#[tauri::command]
pub fn remove_provider(account_id: i64) -> Result<ProviderInfo, String> {
    let acc = get_account(account_id)?;
    if acc.get("type").and_then(|t| t.as_str()) != Some("apikey") {
        return Err("只能删除 type=apikey 的供应商账号（不会动 oauth/卡密）".into());
    }
    let info = account_to_info(&acc).ok_or_else(|| "无法解析账号".to_string())?;
    let prefix = info.prefix.clone();

    admin_json(
        reqwest::Method::DELETE,
        &format!("/api/v1/admin/accounts/{account_id}"),
        None,
    )?;

    if !prefix.is_empty() {
        let _ = remove_catalog_prefix(&prefix)?;
    }
    crate::http_util::invalidate_cache("gateway_status");
    Ok(info)
}

/// Re-probe upstream models, refresh Sub2API mapping + Codex catalog.
#[tauri::command]
pub fn sync_provider_models(
    account_id: i64,
    api_key: Option<String>,
) -> Result<ProviderMutationResult, String> {
    let acc = get_account(account_id)?;
    if acc.get("type").and_then(|t| t.as_str()) != Some("apikey") {
        return Err("只能同步 type=apikey 账号".into());
    }
    let mut info = account_to_info(&acc).ok_or_else(|| "无法解析账号".to_string())?;
    if info.base_url.is_empty() {
        return Err("账号缺少 credentials.base_url".into());
    }

    let model_ids = if let Some(key) = api_key.as_ref().map(|s| s.trim()).filter(|s| !s.is_empty())
    {
        probe_upstream_models(&info.base_url, key)?
    } else {
        match sync_upstream_via_admin(account_id) {
            Ok(ids) => ids,
            Err(admin_err) => {
                return Err(format!(
                    "{admin_err}。也可在同步时重新粘贴 API Key 做直连探测。"
                ));
            }
        }
    };

    let mut prefix = info.prefix.clone();
    if prefix.is_empty() {
        prefix = slugify_prefix(&info.name);
    }
    let mapping = build_model_mapping(&prefix, &model_ids);
    let updated = update_account_mapping(account_id, &info.base_url, &mapping)?;
    info = account_to_info(&updated).unwrap_or(ProviderInfo {
        prefix: prefix.clone(),
        model_count: model_ids.len() as u32,
        ..info
    });

    let models_synced = upsert_catalog_models(&info.name, &prefix, &model_ids)?;
    let host = host_from_base_url(&info.base_url).unwrap_or_default();
    let (allowlist_updated, restart_required, hint) = try_add_upstream_host_allowlist(&host)?;

    Ok(ProviderMutationResult {
        provider: ProviderInfo {
            prefix,
            model_count: model_ids.len() as u32,
            ..info
        },
        models_synced,
        model_ids,
        allowlist_updated,
        restart_required,
        hint,
    })
}

/// Dry-run upstream /models probe (does not create accounts or write catalog).
#[tauri::command]
pub fn probe_provider_models(base_url: String, api_key: String) -> Result<Vec<String>, String> {
    let base_url = normalize_base_url(&base_url)?;
    probe_upstream_models(&base_url, &api_key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_and_slug() {
        assert_eq!(
            normalize_base_url("aihub.top").unwrap(),
            "https://aihub.top/v1"
        );
        assert_eq!(
            normalize_base_url("https://relay.example.com/v1/").unwrap(),
            "https://relay.example.com/v1"
        );
        assert_eq!(slugify_prefix("My Relay!"), "my-relay");
        assert_eq!(validate_prefix("my-relay").unwrap(), "my-relay");
    }

    #[test]
    #[ignore = "requires the live local Sub2API deployment"]
    fn live_list_providers() {
        let list = list_providers().expect("list");
        println!("providers={}", list.len());
        for p in &list {
            println!(
                "id={} name={} prefix={} models={} status={} url={}",
                p.id, p.name, p.prefix, p.model_count, p.status, p.base_url_masked
            );
        }
        assert!(list.iter().any(|p| p.name == "AIHub"));
    }
}
