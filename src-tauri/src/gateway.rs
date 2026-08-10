//! Local Codex multi-provider gateway control (Docker Sub2API).

use crate::http_util::{friendly_http_err, now_iso, HTTP};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const GATEWAY_BASE: &str = "http://127.0.0.1:18080";
const GATEWAY_PORT: u16 = 18080;

/// Resolve the local Sub2API install directory.
/// Preference: `SUB2API_DIR` env → `CODEX_PROVIDER_HUB_SUB2API_DIR` →
/// `$HOME/Documents/Codex/sub2api-ready`.
pub fn sub2api_dir() -> PathBuf {
    for key in ["SUB2API_DIR", "CODEX_PROVIDER_HUB_SUB2API_DIR"] {
        if let Ok(p) = std::env::var(key) {
            let p = p.trim();
            if !p.is_empty() {
                return PathBuf::from(p);
            }
        }
    }
    dirs::home_dir()
        .unwrap_or_default()
        .join("Documents/Codex/sub2api-ready")
}

/// Runtime status of the local Codex provider gateway.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewayStatus {
    pub running: bool,
    pub healthy: bool,
    pub port: u16,
    pub provider_count: u32,
    pub model_count: u32,
    pub last_checked_at: String,
}

/// A single model route entry in the gateway config.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelRoute {
    pub id: String,
    pub prefix: String,
    pub upstream: String,
    pub enabled: bool,
}

/// Full provider configuration loaded by the gateway.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConfig {
    pub listen_addr: String,
    pub providers: Vec<String>,
    pub models: Vec<ModelRoute>,
    pub updated_at: String,
}

fn gateway_key_path() -> PathBuf {
    sub2api_dir().join("state/gateway-api-key")
}

pub fn read_gateway_key() -> Result<String, String> {
    let path = gateway_key_path();
    let key = fs::read_to_string(&path)
        .map_err(|e| format!("read gateway key ({}): {e}", path.display()))?;
    let key = key.trim().to_string();
    if key.is_empty() {
        return Err("gateway API key file is empty".into());
    }
    Ok(key)
}

fn run_sub2api(args: &[&str]) -> Result<String, String> {
    let dir = sub2api_dir();
    if !dir.is_dir() {
        return Err(format!("sub2api directory missing: {}", dir.display()));
    }
    let output = Command::new("./sub2api")
        .args(args)
        .current_dir(&dir)
        .output()
        .map_err(|e| format!("spawn ./sub2api {:?}: {e}", args))?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if !output.status.success() {
        let msg = if stderr.trim().is_empty() {
            stdout.trim().to_string()
        } else {
            stderr.trim().to_string()
        };
        return Err(format!("./sub2api {} failed: {msg}", args.join(" ")));
    }
    Ok(stdout)
}

fn probe_health() -> bool {
    match HTTP.get(format!("{GATEWAY_BASE}/health")).send() {
        Ok(resp) if resp.status().is_success() => resp
            .json::<serde_json::Value>()
            .ok()
            .and_then(|v| v.get("status").and_then(|s| s.as_str()).map(|s| s == "ok"))
            .unwrap_or(true),
        _ => false,
    }
}

fn docker_compose_running() -> bool {
    match run_sub2api(&["status"]) {
        Ok(out) => {
            // STATUS column contains "Up" when containers are running.
            out.lines().any(|l| l.contains("Up ") || l.contains("(healthy)"))
                || out.contains(r#""status":"ok""#)
        }
        Err(_) => false,
    }
}

fn count_models_via_api() -> Result<u32, String> {
    let key = read_gateway_key()?;
    let resp = HTTP
        .get(format!("{GATEWAY_BASE}/v1/models"))
        .bearer_auth(&key)
        .send()
        .map_err(|e| friendly_http_err("GET /v1/models", e))?;
    if !resp.status().is_success() {
        return Err(format!("GET /v1/models HTTP {}", resp.status()));
    }
    let body: serde_json::Value = resp
        .json()
        .map_err(|e| format!("parse /v1/models: {e}"))?;
    let data = body
        .get("data")
        .and_then(|d| d.as_array())
        .ok_or_else(|| " /v1/models missing data[]".to_string())?;
    Ok(data.len() as u32)
}

fn wait_until_healthy(timeout: Duration) -> bool {
    let start = std::time::Instant::now();
    while start.elapsed() < timeout {
        if probe_health() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(400));
    }
    false
}

pub(crate) fn codex_config_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_default()
        .join(".codex/config.toml")
}

pub(crate) fn catalog_path_from_config(doc: &toml::Value) -> PathBuf {
    doc.get("model_catalog_json")
        .and_then(|v| v.as_str())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_default()
                .join(".codex/model-catalogs/aihub-sub2api.json")
        })
}

pub(crate) fn backup_file(path: &Path) -> Result<PathBuf, String> {
    if !path.exists() {
        return Err(format!("cannot backup missing file: {}", path.display()));
    }
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let bak = PathBuf::from(format!("{}.bak-{ts}", path.display()));
    fs::copy(path, &bak).map_err(|e| format!("backup {}: {e}", path.display()))?;
    Ok(bak)
}

/// Update `base_url` inside `[model_providers.sub2api]` without rewriting the whole TOML.
fn patch_sub2api_base_url(raw: &str, base_url: &str) -> Result<String, String> {
    let marker = "[model_providers.sub2api]";
    let start = raw
        .find(marker)
        .ok_or_else(|| "config.toml missing [model_providers.sub2api]".to_string())?;
    let after = start + marker.len();
    let rest = &raw[after..];
    let end_rel = rest
        .find("\n[")
        .map(|i| after + i)
        .unwrap_or(raw.len());
    let section = &raw[start..end_rel];
    if !section.contains("base_url") {
        return Err("[model_providers.sub2api] has no base_url to update".into());
    }
    let mut replaced_section = String::new();
    let mut done = false;
    for line in section.lines() {
        if !done && line.trim_start().starts_with("base_url") {
            let indent = &line[..line.len() - line.trim_start().len()];
            replaced_section.push_str(indent);
            replaced_section.push_str(&format!("base_url = \"{base_url}\""));
            done = true;
        } else {
            replaced_section.push_str(line);
        }
        replaced_section.push('\n');
    }
    // Preserve whether original section ended without trailing newline awkwardly.
    if !section.ends_with('\n') && replaced_section.ends_with('\n') {
        replaced_section.pop();
    }
    let mut out = String::with_capacity(raw.len() + 16);
    out.push_str(&raw[..start]);
    out.push_str(&replaced_section);
    out.push_str(&raw[end_rel..]);
    Ok(out)
}

fn route_from_slug(slug: &str, enabled: bool) -> ModelRoute {
    let (prefix, upstream) = if slug.starts_with("aihub-") {
        ("aihub-", "aihub")
    } else if slug.starts_with("sub2api-") {
        ("sub2api-", "sub2api")
    } else {
        ("", "unknown")
    };
    ModelRoute {
        id: slug.to_string(),
        prefix: prefix.into(),
        upstream: upstream.into(),
        enabled,
    }
}

fn load_provider_config_from_disk() -> Result<ProviderConfig, String> {
    let cfg_path = codex_config_path();
    let raw = fs::read_to_string(&cfg_path)
        .map_err(|e| format!("read {}: {e}", cfg_path.display()))?;
    let doc: toml::Value =
        toml::from_str(&raw).map_err(|e| format!("parse config.toml: {e}"))?;

    let listen_addr = doc
        .get("model_providers")
        .and_then(|p| p.get("sub2api"))
        .and_then(|s| s.get("base_url"))
        .and_then(|u| u.as_str())
        .map(|url| {
            url.trim_start_matches("http://")
                .trim_start_matches("https://")
                .trim_end_matches("/v1")
                .trim_end_matches('/')
                .to_string()
        })
        .unwrap_or_else(|| format!("127.0.0.1:{GATEWAY_PORT}"));

    let catalog_path = catalog_path_from_config(&doc);
    let catalog_raw = fs::read_to_string(&catalog_path)
        .map_err(|e| format!("read catalog {}: {e}", catalog_path.display()))?;
    let catalog: serde_json::Value =
        serde_json::from_str(&catalog_raw).map_err(|e| format!("parse catalog: {e}"))?;
    let models = catalog
        .get("models")
        .and_then(|m| m.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| {
                    let slug = m.get("slug")?.as_str()?;
                    let enabled = m
                        .get("visibility")
                        .and_then(|v| v.as_str())
                        .map(|v| v != "hidden")
                        .unwrap_or(true);
                    Some(route_from_slug(slug, enabled))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let mtime = fs::metadata(&cfg_path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| {
            let dt: chrono::DateTime<chrono::Utc> = t.into();
            Some(dt.to_rfc3339())
        })
        .unwrap_or_else(now_iso);

    Ok(ProviderConfig {
        listen_addr,
        providers: vec!["aihub".into(), "sub2api".into()],
        models,
        updated_at: mtime,
    })
}

/// Probe gateway health / docker status / model counts.
#[tauri::command]
pub fn get_gateway_status() -> Result<GatewayStatus, String> {
    crate::http_util::cached_json("gateway_status", Duration::from_secs(5), || {
        let healthy = probe_health();
        let running = healthy || docker_compose_running();
        let model_count = if healthy {
            count_models_via_api().unwrap_or(0)
        } else {
            load_provider_config_from_disk()
                .map(|c| c.models.len() as u32)
                .unwrap_or(0)
        };
        Ok(GatewayStatus {
            running,
            healthy,
            port: GATEWAY_PORT,
            provider_count: 2,
            model_count,
            last_checked_at: now_iso(),
        })
    })
}

/// Start Docker Sub2API via `./sub2api up`.
#[tauri::command]
pub fn start_gateway() -> Result<GatewayStatus, String> {
    crate::http_util::invalidate_cache("gateway_status");
    if probe_health() {
        return get_gateway_status();
    }
    run_sub2api(&["up"])?;
    if !wait_until_healthy(Duration::from_secs(45)) {
        return Err("网关已启动命令，但 /health 在 45s 内未就绪".into());
    }
    crate::http_util::invalidate_cache("gateway_status");
    get_gateway_status()
}

/// Stop Docker Sub2API via `./sub2api down`.
#[tauri::command]
pub fn stop_gateway() -> Result<GatewayStatus, String> {
    crate::http_util::invalidate_cache("gateway_status");
    run_sub2api(&["down"])?;
    // Give containers a moment to exit.
    std::thread::sleep(Duration::from_secs(2));
    crate::http_util::invalidate_cache("gateway_status");
    get_gateway_status()
}

/// Read provider routing from `~/.codex/config.toml` + model catalog.
#[tauri::command]
pub fn get_provider_config() -> Result<ProviderConfig, String> {
    load_provider_config_from_disk()
}

/// Persist safe fields only: backup first, never rename `model_providers.sub2api`.
#[tauri::command]
pub fn save_provider_config(cfg: ProviderConfig) -> Result<ProviderConfig, String> {
    let cfg_path = codex_config_path();
    let raw = fs::read_to_string(&cfg_path)
        .map_err(|e| format!("read {}: {e}", cfg_path.display()))?;
    let doc: toml::Value =
        toml::from_str(&raw).map_err(|e| format!("parse config.toml: {e}"))?;

    // Refuse to proceed if provider table is missing — never invent a new id.
    let has_sub2 = doc
        .get("model_providers")
        .and_then(|v| v.get("sub2api"))
        .is_some();
    if !has_sub2 {
        return Err("config.toml missing [model_providers.sub2api] — refusing to create".into());
    }

    let catalog_path = catalog_path_from_config(&doc);
    let _cfg_bak = backup_file(&cfg_path)?;
    let _cat_bak = backup_file(&catalog_path)?;

    // Safe field: base_url derived from listenAddr (surgical patch keeps comments).
    let listen = cfg
        .listen_addr
        .trim()
        .trim_start_matches("http://")
        .trim_start_matches("https://");
    let base_url = if listen.ends_with("/v1") {
        format!("http://{listen}")
    } else {
        format!("http://{}/v1", listen.trim_end_matches('/'))
    };
    let patched = patch_sub2api_base_url(&raw, &base_url)?;
    fs::write(&cfg_path, patched).map_err(|e| format!("write config.toml: {e}"))?;

    // Sync catalog visibility from enabled flags; do not add/remove model entries.
    let catalog_raw =
        fs::read_to_string(&catalog_path).map_err(|e| format!("read catalog: {e}"))?;
    let mut catalog: serde_json::Value =
        serde_json::from_str(&catalog_raw).map_err(|e| format!("parse catalog: {e}"))?;
    let enabled_map: std::collections::HashMap<&str, bool> = cfg
        .models
        .iter()
        .map(|m| (m.id.as_str(), m.enabled))
        .collect();
    if let Some(models) = catalog.get_mut("models").and_then(|m| m.as_array_mut()) {
        for model in models {
            if let Some(slug) = model.get("slug").and_then(|s| s.as_str()) {
                if let Some(&enabled) = enabled_map.get(slug) {
                    let vis = if enabled { "list" } else { "hidden" };
                    if let Some(obj) = model.as_object_mut() {
                        obj.insert("visibility".into(), serde_json::json!(vis));
                    }
                }
            }
        }
    }
    let catalog_out =
        serde_json::to_string_pretty(&catalog).map_err(|e| format!("serialize catalog: {e}"))?;
    fs::write(&catalog_path, catalog_out + "\n")
        .map_err(|e| format!("write catalog: {e}"))?;

    crate::http_util::invalidate_cache("gateway_status");
    let mut saved = load_provider_config_from_disk()?;
    saved.updated_at = now_iso();
    Ok(saved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_health_and_models() {
        let healthy = probe_health();
        println!("healthy={healthy}");
        if healthy {
            let n = count_models_via_api().expect("models");
            println!("model_count={n}");
            assert!(n > 0);
        }
        let cfg = load_provider_config_from_disk().expect("config");
        println!(
            "listen={} providers={:?} catalog_models={}",
            cfg.listen_addr,
            cfg.providers,
            cfg.models.len()
        );
        assert_eq!(cfg.providers.len(), 2);
        assert!(cfg.models.len() >= 10);
    }
}
