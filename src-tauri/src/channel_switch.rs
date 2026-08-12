//! Atomic Codex channel profiles: official ChatGPT OAuth <-> Sub2API.
//!
//! This module deliberately keeps credentials inside `auth.json`.  The profile
//! store contains routing metadata only, and command responses never include
//! tokens or API keys.

use chrono::Utc;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Map as JsonMap, Value as JsonValue};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;
use uuid::Uuid;

const PROFILE_FILE: &str = ".provider-hub-channel-profiles.json";
const DEFAULT_OFFICIAL_MODEL: &str = "gpt-5.6-sol";
const SUB2API_PREFIX: &str = "sub2api-";

/// Prevent overlapping UI invocations from interleaving the two-file commit.
static CHANNEL_FILES_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ChannelTarget {
    Official,
    #[serde(rename = "sub2api")]
    Sub2Api,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ActiveChannel {
    Official,
    #[serde(rename = "sub2api")]
    Sub2Api,
    Mixed,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelSwitchStatus {
    pub current: ActiveChannel,
    pub model_provider: String,
    pub model: String,
    pub auth_mode: String,
    pub preferred_auth_method: String,
    pub official_profile_saved: bool,
    pub sub2api_profile_saved: bool,
    pub last_switched_at: Option<String>,
    pub config_consistent: bool,
    pub official_account: Option<OfficialAccountInfo>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OfficialAccountInfo {
    pub email: String,
    pub name: Option<String>,
    pub picture: Option<String>,
    pub has_active_subscription: bool,
    pub subscription_plan: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelSwitchResult {
    pub status: ChannelSwitchStatus,
    pub auth_backup_path: String,
    pub config_backup_path: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexRestartResult {
    pub application: String,
    pub reopened: bool,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChannelProfile {
    model_provider: String,
    model: String,
    auth_mode: String,
    preferred_auth_method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    openai_base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    model_catalog_json: Option<String>,
    saved_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChannelProfiles {
    version: u8,
    #[serde(default)]
    official: Option<ChannelProfile>,
    #[serde(default, rename = "sub2api")]
    sub2api: Option<ChannelProfile>,
    #[serde(default)]
    last_switched_at: Option<String>,
}

impl Default for ChannelProfiles {
    fn default() -> Self {
        Self {
            version: 1,
            official: None,
            sub2api: None,
            last_switched_at: None,
        }
    }
}

#[derive(Debug, Clone)]
struct ChannelPaths {
    auth: PathBuf,
    config: PathBuf,
    profiles: PathBuf,
}

impl ChannelPaths {
    fn for_dir(codex_dir: &Path) -> Self {
        Self {
            auth: codex_dir.join("auth.json"),
            config: codex_dir.join("config.toml"),
            profiles: codex_dir.join(PROFILE_FILE),
        }
    }

    fn live() -> Result<Self, String> {
        let home = dirs::home_dir().ok_or_else(|| "无法定位用户主目录".to_string())?;
        Ok(Self::for_dir(&home.join(".codex")))
    }
}

fn safe_read(path: &Path, label: &str) -> Result<Vec<u8>, String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|e| format!("读取 {label} 失败（{}）：{e}", path.display()))?;
    if metadata.file_type().is_symlink() {
        return Err(format!(
            "拒绝读取符号链接形式的 {label}：{}",
            path.display()
        ));
    }
    if !metadata.is_file() {
        return Err(format!("{label} 不是普通文件：{}", path.display()));
    }
    fs::read(path).map_err(|e| format!("读取 {label} 失败（{}）：{e}", path.display()))
}

fn parse_auth(bytes: &[u8]) -> Result<JsonValue, String> {
    let value: JsonValue =
        serde_json::from_slice(bytes).map_err(|e| format!("auth.json 格式无效：{e}"))?;
    if !value.is_object() {
        return Err("auth.json 顶层必须是 JSON object".into());
    }
    Ok(value)
}

fn parse_config(bytes: &[u8]) -> Result<toml::Value, String> {
    let raw = std::str::from_utf8(bytes).map_err(|e| format!("config.toml 不是 UTF-8：{e}"))?;
    let value: toml::Value =
        toml::from_str(raw).map_err(|e| format!("config.toml 格式无效：{e}"))?;
    if !value.is_table() {
        return Err("config.toml 顶层必须是 table".into());
    }
    Ok(value)
}

fn json_string<'a>(value: &'a JsonValue, key: &str) -> &'a str {
    value.get(key).and_then(JsonValue::as_str).unwrap_or("")
}

fn toml_string<'a>(value: &'a toml::Value, key: &str) -> &'a str {
    value.get(key).and_then(toml::Value::as_str).unwrap_or("")
}

fn active_channel(auth: &JsonValue, config: &toml::Value) -> ActiveChannel {
    match (
        toml_string(config, "model_provider"),
        json_string(auth, "auth_mode"),
    ) {
        ("openai", "chatgpt") => ActiveChannel::Official,
        ("sub2api", "apikey") => ActiveChannel::Sub2Api,
        _ => ActiveChannel::Mixed,
    }
}

fn has_nonempty_string(value: Option<&JsonValue>) -> bool {
    value
        .and_then(JsonValue::as_str)
        .map(str::trim)
        .map(|s| !s.is_empty())
        .unwrap_or(false)
}

fn validate_target_credentials(auth: &JsonValue, target: ChannelTarget) -> Result<(), String> {
    match target {
        ChannelTarget::Official => {
            let tokens = auth.get("tokens").and_then(JsonValue::as_object);
            let has_access = tokens
                .map(|t| has_nonempty_string(t.get("access_token")))
                .unwrap_or(false);
            let has_refresh = tokens
                .map(|t| has_nonempty_string(t.get("refresh_token")))
                .unwrap_or(false);
            if !has_access && !has_refresh {
                return Err("未找到官方 ChatGPT OAuth 凭据，请先完成浏览器登录".into());
            }
        }
        ChannelTarget::Sub2Api => {
            if !has_nonempty_string(auth.get("OPENAI_API_KEY")) {
                return Err("未找到 Sub2API 网关 API key，请先配置 OPENAI_API_KEY".into());
            }
        }
    }
    Ok(())
}

fn nested_sub2api_base_url(config: &toml::Value) -> Option<String> {
    config
        .get("model_providers")
        .and_then(|v| v.get("sub2api"))
        .and_then(|v| v.get("base_url"))
        .and_then(toml::Value::as_str)
        .map(ToOwned::to_owned)
}

fn usable_http_base_url(value: &str) -> bool {
    reqwest::Url::parse(value.trim())
        .ok()
        .is_some_and(|url| matches!(url.scheme(), "http" | "https") && url.host_str().is_some())
}

fn has_gateway_model_prefix(model: &str) -> bool {
    ["sub2api-", "aihub-", "anyrouter-"]
        .iter()
        .any(|prefix| model.starts_with(prefix))
}

fn capture_profile(
    channel: ActiveChannel,
    auth: &JsonValue,
    config: &toml::Value,
) -> Option<ChannelProfile> {
    if channel == ActiveChannel::Mixed {
        return None;
    }
    Some(ChannelProfile {
        model_provider: toml_string(config, "model_provider").to_string(),
        model: toml_string(config, "model").to_string(),
        auth_mode: json_string(auth, "auth_mode").to_string(),
        preferred_auth_method: toml_string(config, "preferred_auth_method").to_string(),
        openai_base_url: config
            .get("openai_base_url")
            .and_then(toml::Value::as_str)
            .map(ToOwned::to_owned)
            .or_else(|| {
                (channel == ActiveChannel::Sub2Api)
                    .then(|| nested_sub2api_base_url(config))
                    .flatten()
            }),
        model_catalog_json: config
            .get("model_catalog_json")
            .and_then(toml::Value::as_str)
            .map(ToOwned::to_owned),
        saved_at: Utc::now().to_rfc3339(),
    })
}

fn strip_channel_prefix(model: &str) -> String {
    let trimmed = model.trim();
    for prefix in ["sub2api-", "aihub-", "anyrouter-"] {
        if let Some(real) = trimmed.strip_prefix(prefix) {
            if !real.is_empty() {
                return real.to_string();
            }
        }
    }
    if trimmed.is_empty() {
        DEFAULT_OFFICIAL_MODEL.to_string()
    } else {
        trimmed.to_string()
    }
}

fn gateway_model(model: &str) -> String {
    format!("{SUB2API_PREFIX}{}", strip_channel_prefix(model))
}

fn load_profiles(path: &Path) -> Result<ChannelProfiles, String> {
    if !path.exists() {
        return Ok(ChannelProfiles::default());
    }
    let bytes = safe_read(path, "通道 profile")?;
    let profiles: ChannelProfiles =
        serde_json::from_slice(&bytes).map_err(|e| format!("通道 profile 格式无效：{e}"))?;
    if profiles.version != 1 {
        return Err(format!("不支持的通道 profile 版本：{}", profiles.version));
    }
    Ok(profiles)
}

fn serialize_profiles(profiles: &ChannelProfiles) -> Result<Vec<u8>, String> {
    let mut bytes =
        serde_json::to_vec_pretty(profiles).map_err(|e| format!("序列化通道 profile 失败：{e}"))?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn serialize_auth(auth: &JsonValue) -> Result<Vec<u8>, String> {
    let mut bytes =
        serde_json::to_vec_pretty(auth).map_err(|e| format!("序列化 auth.json 失败：{e}"))?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn serialize_config(config: &toml::Value) -> Result<Vec<u8>, String> {
    let mut raw =
        toml::to_string_pretty(config).map_err(|e| format!("序列化 config.toml 失败：{e}"))?;
    if !raw.ends_with('\n') {
        raw.push('\n');
    }
    Ok(raw.into_bytes())
}

fn set_json_string(auth: &mut JsonValue, key: &str, value: &str) -> Result<(), String> {
    let object: &mut JsonMap<String, JsonValue> = auth
        .as_object_mut()
        .ok_or_else(|| "auth.json 顶层必须是 JSON object".to_string())?;
    object.insert(key.to_string(), JsonValue::String(value.to_string()));
    Ok(())
}

fn config_table_mut(
    config: &mut toml::Value,
) -> Result<&mut toml::map::Map<String, toml::Value>, String> {
    config
        .as_table_mut()
        .ok_or_else(|| "config.toml 顶层必须是 table".to_string())
}

fn set_config_string(config: &mut toml::Value, key: &str, value: &str) -> Result<(), String> {
    config_table_mut(config)?.insert(key.into(), toml::Value::String(value.into()));
    Ok(())
}

fn remove_config_key(config: &mut toml::Value, key: &str) -> Result<(), String> {
    config_table_mut(config)?.remove(key);
    Ok(())
}

fn apply_target(
    target: ChannelTarget,
    auth: &mut JsonValue,
    config: &mut toml::Value,
    profiles: &ChannelProfiles,
    codex_dir: &Path,
) -> Result<ChannelProfile, String> {
    let now = Utc::now().to_rfc3339();
    match target {
        ChannelTarget::Official => {
            let source_model = profiles
                .official
                .as_ref()
                .map(|p| p.model.as_str())
                .filter(|m| !m.trim().is_empty())
                .unwrap_or_else(|| toml_string(config, "model"));
            let model = strip_channel_prefix(source_model);
            set_json_string(auth, "auth_mode", "chatgpt")?;
            set_config_string(config, "model_provider", "openai")?;
            set_config_string(config, "model", &model)?;
            set_config_string(config, "preferred_auth_method", "chatgpt")?;
            remove_config_key(config, "openai_base_url")?;
            remove_config_key(config, "model_catalog_json")?;
            Ok(ChannelProfile {
                model_provider: "openai".into(),
                model,
                auth_mode: "chatgpt".into(),
                preferred_auth_method: "chatgpt".into(),
                openai_base_url: None,
                model_catalog_json: None,
                saved_at: now,
            })
        }
        ChannelTarget::Sub2Api => {
            let source_model = profiles
                .sub2api
                .as_ref()
                .map(|p| p.model.as_str())
                .filter(|m| !m.trim().is_empty())
                .unwrap_or_else(|| toml_string(config, "model"));
            let model = gateway_model(source_model);
            let provider_base_url = nested_sub2api_base_url(config).ok_or_else(|| {
                "config.toml 缺少可用的 [model_providers.sub2api].base_url，无法切换到 Sub2API"
                    .to_string()
            })?;
            if !usable_http_base_url(&provider_base_url) {
                return Err(
                    "[model_providers.sub2api].base_url 必须是有效的 http(s) URL".to_string(),
                );
            }
            let saved_base_url = profiles
                .sub2api
                .as_ref()
                .and_then(|p| p.openai_base_url.clone())
                .unwrap_or(provider_base_url);
            if !usable_http_base_url(&saved_base_url) {
                return Err("Sub2API profile 中的 openai_base_url 不是有效的 http(s) URL".into());
            }
            let saved_catalog = profiles
                .sub2api
                .as_ref()
                .and_then(|p| p.model_catalog_json.clone())
                .or_else(|| {
                    let default = codex_dir.join("model-catalogs/aihub-sub2api.json");
                    default
                        .is_file()
                        .then(|| default.to_string_lossy().into_owned())
                });

            set_json_string(auth, "auth_mode", "apikey")?;
            set_config_string(config, "model_provider", "sub2api")?;
            set_config_string(config, "model", &model)?;
            set_config_string(config, "preferred_auth_method", "apikey")?;
            set_config_string(config, "openai_base_url", &saved_base_url)?;
            if let Some(ref path) = saved_catalog {
                set_config_string(config, "model_catalog_json", path)?;
            } else {
                remove_config_key(config, "model_catalog_json")?;
            }
            Ok(ChannelProfile {
                model_provider: "sub2api".into(),
                model,
                auth_mode: "apikey".into(),
                preferred_auth_method: "apikey".into(),
                openai_base_url: Some(saved_base_url),
                model_catalog_json: saved_catalog,
                saved_at: now,
            })
        }
    }
}

fn unique_temp_path(path: &Path) -> Result<PathBuf, String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("路径缺少父目录：{}", path.display()))?;
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| format!("无效文件名：{}", path.display()))?;
    Ok(parent.join(format!(
        ".{name}.tmp-{}-{}",
        std::process::id(),
        Uuid::new_v4().simple()
    )))
}

fn file_mode(path: &Path) -> u32 {
    #[cfg(unix)]
    {
        return fs::metadata(path)
            .map(|m| m.permissions().mode() & 0o777)
            .unwrap_or(0o600);
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        0
    }
}

fn stage_file(path: &Path, bytes: &[u8]) -> Result<PathBuf, String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("路径缺少父目录：{}", path.display()))?;
    fs::create_dir_all(parent).map_err(|e| format!("创建目录 {} 失败：{e}", parent.display()))?;
    let temp = unique_temp_path(path)?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(file_mode(path));
    let mut file = options
        .open(&temp)
        .map_err(|e| format!("创建临时文件 {} 失败：{e}", temp.display()))?;
    if let Err(e) = file.write_all(bytes).and_then(|_| file.sync_all()) {
        let _ = fs::remove_file(&temp);
        return Err(format!("写入临时文件 {} 失败：{e}", temp.display()));
    }
    Ok(temp)
}

fn atomic_replace(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let temp = stage_file(path, bytes)?;
    if let Err(e) = fs::rename(&temp, path) {
        let _ = fs::remove_file(&temp);
        return Err(format!("原子替换 {} 失败：{e}", path.display()));
    }
    sync_parent(path);
    Ok(())
}

fn sync_parent(path: &Path) {
    if let Some(parent) = path.parent() {
        if let Ok(dir) = File::open(parent) {
            let _ = dir.sync_all();
        }
    }
}

fn backup_path(path: &Path, purpose: &str, stamp: &str) -> PathBuf {
    PathBuf::from(format!("{}.bak-{purpose}-{stamp}", path.display()))
}

fn create_backup(
    path: &Path,
    original: &[u8],
    purpose: &str,
    stamp: &str,
    private: bool,
) -> Result<PathBuf, String> {
    let backup = backup_path(path, purpose, stamp);
    if backup.exists() {
        return Err(format!("备份文件已存在：{}", backup.display()));
    }
    atomic_replace(&backup, original)?;
    #[cfg(unix)]
    fs::set_permissions(
        &backup,
        fs::Permissions::from_mode(if private { 0o600 } else { file_mode(path) }),
    )
    .map_err(|e| format!("设置备份权限失败（{}）：{e}", backup.display()))?;
    Ok(backup)
}

#[derive(Clone, Copy)]
enum CommitMode {
    Normal,
    #[cfg(test)]
    FailAfterFirst,
    #[cfg(test)]
    ConcurrentAuthBeforeCommit,
    #[cfg(test)]
    ConcurrentConfigAfterFirst,
}

fn ensure_file_unchanged(path: &Path, expected: &[u8], label: &str) -> Result<(), String> {
    let current = safe_read(path, label)?;
    if current != expected {
        return Err(format!(
            "检测到 Codex 并发修改了 {label}；为避免覆盖新凭据，本次切换已中止"
        ));
    }
    Ok(())
}

fn rollback_if_unchanged(
    path: &Path,
    committed: &[u8],
    original: &[u8],
    label: &str,
) -> Result<bool, String> {
    let current = safe_read(path, label)?;
    if current != committed {
        return Ok(false);
    }
    atomic_replace(path, original)?;
    Ok(true)
}

fn commit_pair(
    paths: &ChannelPaths,
    old_auth: &[u8],
    old_config: &[u8],
    new_auth: &[u8],
    new_config: &[u8],
    mode: CommitMode,
) -> Result<(), String> {
    let auth_temp = stage_file(&paths.auth, new_auth)?;
    let config_temp = match stage_file(&paths.config, new_config) {
        Ok(path) => path,
        Err(e) => {
            let _ = fs::remove_file(&auth_temp);
            return Err(e);
        }
    };

    #[cfg(test)]
    if matches!(mode, CommitMode::ConcurrentAuthBeforeCommit) {
        atomic_replace(
            &paths.auth,
            br#"{"auth_mode":"chatgpt","tokens":{"refresh_token":"rotated"}}
"#,
        )?;
    }

    if let Err(error) = ensure_file_unchanged(&paths.config, old_config, "config.toml")
        .and_then(|_| ensure_file_unchanged(&paths.auth, old_auth, "auth.json"))
    {
        let _ = fs::remove_file(&auth_temp);
        let _ = fs::remove_file(&config_temp);
        return Err(error);
    }

    if let Err(e) = fs::rename(&auth_temp, &paths.auth) {
        let _ = fs::remove_file(&auth_temp);
        let _ = fs::remove_file(&config_temp);
        return Err(format!("提交 auth.json 失败：{e}"));
    }
    sync_parent(&paths.auth);

    #[cfg(test)]
    if matches!(mode, CommitMode::ConcurrentConfigAfterFirst) {
        atomic_replace(&paths.config, b"# concurrently changed\n")?;
    }

    #[cfg(test)]
    if matches!(mode, CommitMode::FailAfterFirst) {
        let _ = fs::remove_file(&config_temp);
        atomic_replace(&paths.auth, old_auth)
            .map_err(|rollback| format!("模拟提交失败，且 auth.json 回滚失败：{rollback}"))?;
        return Err("模拟 config.toml 提交失败；auth.json 已回滚".into());
    }

    if let Err(error) = ensure_file_unchanged(&paths.auth, new_auth, "auth.json")
        .and_then(|_| ensure_file_unchanged(&paths.config, old_config, "config.toml"))
    {
        let _ = fs::remove_file(&config_temp);
        match rollback_if_unchanged(&paths.auth, new_auth, old_auth, "auth.json") {
            Ok(true) => {
                return Err(format!("{error}；auth.json 已自动回滚"));
            }
            Ok(false) => {
                return Err(format!(
                    "{error}；auth.json 也被外部修改，为保护新凭据未自动覆盖，请检查当前配置"
                ));
            }
            Err(rollback) => {
                return Err(format!(
                    "{error}；auth.json 回滚失败：{rollback}（请从备份恢复）"
                ));
            }
        }
    }
    #[cfg(not(test))]
    let _ = mode;

    if let Err(e) = fs::rename(&config_temp, &paths.config) {
        let _ = fs::remove_file(&config_temp);
        match rollback_if_unchanged(&paths.auth, new_auth, old_auth, "auth.json") {
            Ok(true) => return Err(format!("提交 config.toml 失败：{e}；auth.json 已自动回滚")),
            Ok(false) => {
                return Err(format!(
                    "提交 config.toml 失败：{e}；auth.json 已被外部修改，为保护新凭据未自动覆盖"
                ));
            }
            Err(rollback) => {
                return Err(format!(
                    "提交 config.toml 失败：{e}；auth.json 回滚也失败：{rollback}（请从备份恢复）"
                ));
            }
        }
    }
    sync_parent(&paths.config);
    Ok(())
}

fn extract_official_account(auth: &JsonValue) -> Option<OfficialAccountInfo> {
    let user = auth.get("user").and_then(JsonValue::as_object)?;
    let email = user.get("email").and_then(JsonValue::as_str)?;

    let name = user.get("name").and_then(JsonValue::as_str).map(String::from);
    let picture = user.get("picture").and_then(JsonValue::as_str).map(String::from);

    // Check for active subscription from accounts array
    let accounts = auth.get("accounts").and_then(JsonValue::as_array);
    let (has_active_subscription, subscription_plan) = if let Some(accounts) = accounts {
        let mut has_sub = false;
        let mut plan_name = None;

        for account in accounts {
            if let Some(obj) = account.as_object() {
                // Check for ChatGPT Plus/Team/Enterprise indicators
                if let Some(account_obj) = obj.get("account").and_then(JsonValue::as_object) {
                    let plan_type = account_obj.get("plan_type").and_then(JsonValue::as_str);

                    if let Some(plan) = plan_type {
                        if plan != "free" && !plan.is_empty() {
                            has_sub = true;
                            plan_name = Some(match plan {
                                "plus" => "ChatGPT Plus",
                                "team" => "ChatGPT Team",
                                "enterprise" => "ChatGPT Enterprise",
                                other => other,
                            }.to_string());
                            break;
                        }
                    }
                }
            }
        }
        (has_sub, plan_name)
    } else {
        (false, None)
    };

    Some(OfficialAccountInfo {
        email: email.to_string(),
        name,
        picture,
        has_active_subscription,
        subscription_plan,
    })
}

fn status_from_docs(
    auth: &JsonValue,
    config: &toml::Value,
    profiles: &ChannelProfiles,
) -> ChannelSwitchStatus {
    let current = active_channel(auth, config);
    let consistent = match current {
        ActiveChannel::Official => {
            let model = toml_string(config, "model").trim();
            config.get("openai_base_url").is_none()
                && config.get("model_catalog_json").is_none()
                && !model.is_empty()
                && !has_gateway_model_prefix(model)
                && toml_string(config, "preferred_auth_method") == "chatgpt"
        }
        ActiveChannel::Sub2Api => {
            let provider_base_url = nested_sub2api_base_url(config);
            let top_level_base_url = config.get("openai_base_url").and_then(toml::Value::as_str);
            toml_string(config, "model").starts_with(SUB2API_PREFIX)
                && toml_string(config, "preferred_auth_method") == "apikey"
                && provider_base_url
                    .as_deref()
                    .is_some_and(usable_http_base_url)
                && top_level_base_url.is_some_and(usable_http_base_url)
        }
        ActiveChannel::Mixed => false,
    };

    let official_account = if current == ActiveChannel::Official {
        extract_official_account(auth)
    } else {
        None
    };

    ChannelSwitchStatus {
        current,
        model_provider: toml_string(config, "model_provider").to_string(),
        model: toml_string(config, "model").to_string(),
        auth_mode: json_string(auth, "auth_mode").to_string(),
        preferred_auth_method: toml_string(config, "preferred_auth_method").to_string(),
        official_profile_saved: profiles.official.is_some(),
        sub2api_profile_saved: profiles.sub2api.is_some(),
        last_switched_at: profiles.last_switched_at.clone(),
        config_consistent: consistent,
        official_account,
    }
}

fn get_status_at(paths: &ChannelPaths) -> Result<ChannelSwitchStatus, String> {
    let auth_bytes = safe_read(&paths.auth, "auth.json")?;
    let config_bytes = safe_read(&paths.config, "config.toml")?;
    let auth = parse_auth(&auth_bytes)?;
    let config = parse_config(&config_bytes)?;
    let profiles = load_profiles(&paths.profiles)?;
    Ok(status_from_docs(&auth, &config, &profiles))
}

fn switch_at(
    paths: &ChannelPaths,
    target: ChannelTarget,
    mode: CommitMode,
) -> Result<ChannelSwitchResult, String> {
    let old_auth = safe_read(&paths.auth, "auth.json")?;
    let old_config = safe_read(&paths.config, "config.toml")?;
    let mut auth = parse_auth(&old_auth)?;
    let mut config = parse_config(&old_config)?;
    validate_target_credentials(&auth, target)?;

    let mut profiles = load_profiles(&paths.profiles)?;
    match capture_profile(active_channel(&auth, &config), &auth, &config) {
        Some(profile) if profile.model_provider == "openai" => profiles.official = Some(profile),
        Some(profile) if profile.model_provider == "sub2api" => profiles.sub2api = Some(profile),
        _ => {}
    }

    let codex_dir = paths
        .config
        .parent()
        .ok_or_else(|| "config.toml 缺少父目录".to_string())?;
    let target_profile = apply_target(target, &mut auth, &mut config, &profiles, codex_dir)?;
    let switched_at = Utc::now().to_rfc3339();
    match target {
        ChannelTarget::Official => profiles.official = Some(target_profile),
        ChannelTarget::Sub2Api => profiles.sub2api = Some(target_profile),
    }
    profiles.last_switched_at = Some(switched_at);

    let new_auth = serialize_auth(&auth)?;
    let new_config = serialize_config(&config)?;
    let new_profiles = serialize_profiles(&profiles)?;

    // Backups are fully durable before either live file is replaced.
    let stamp = Utc::now().format("%Y%m%dT%H%M%S%3fZ").to_string();
    let purpose = match target {
        ChannelTarget::Official => "switch-official",
        ChannelTarget::Sub2Api => "switch-sub2api",
    };
    let auth_backup = create_backup(&paths.auth, &old_auth, purpose, &stamp, true)?;
    let config_backup = create_backup(&paths.config, &old_config, purpose, &stamp, false)?;

    // Pre-stage profile metadata too, so an unwritable profile store cannot
    // leave the two live Codex files switched while reporting failure.
    let profile_temp = stage_file(&paths.profiles, &new_profiles)?;
    #[cfg(unix)]
    if let Err(e) = fs::set_permissions(&profile_temp, fs::Permissions::from_mode(0o600)) {
        let _ = fs::remove_file(&profile_temp);
        return Err(format!("设置通道 profile 临时文件权限失败：{e}"));
    }

    if let Err(e) = commit_pair(paths, &old_auth, &old_config, &new_auth, &new_config, mode) {
        let _ = fs::remove_file(&profile_temp);
        return Err(e);
    }
    if let Err(e) = fs::rename(&profile_temp, &paths.profiles) {
        let _ = fs::remove_file(&profile_temp);
        let auth_rollback = rollback_if_unchanged(&paths.auth, &new_auth, &old_auth, "auth.json");
        let config_rollback =
            rollback_if_unchanged(&paths.config, &new_config, &old_config, "config.toml");
        return match (auth_rollback, config_rollback) {
            (Ok(true), Ok(true)) => Err(format!(
                "提交通道 profile 失败：{e}；auth.json 与 config.toml 已回滚"
            )),
            (Ok(auth_restored), Ok(config_restored)) => Err(format!(
                "提交通道 profile 失败：{e}；检测到外部并发修改，未覆盖新内容（auth 已回滚={auth_restored}，config 已回滚={config_restored}），请检查当前配置"
            )),
            (auth_result, config_result) => Err(format!(
                "提交通道 profile 失败：{e}；回滚不完整（auth={auth_result:?}, config={config_result:?}），请从备份恢复"
            )),
        };
    }
    sync_parent(&paths.profiles);

    let status = status_from_docs(&auth, &config, &profiles);
    let target_label = match target {
        ChannelTarget::Official => "官方直连",
        ChannelTarget::Sub2Api => "Sub2API 网关",
    };
    Ok(ChannelSwitchResult {
        status,
        auth_backup_path: auth_backup.to_string_lossy().into_owned(),
        config_backup_path: config_backup.to_string_lossy().into_owned(),
        message: format!("已切换到 {target_label}；需重启 Codex 应用生效"),
    })
}

#[tauri::command]
pub fn get_channel_switch_status() -> Result<ChannelSwitchStatus, String> {
    let _guard = CHANNEL_FILES_LOCK.lock();
    get_status_at(&ChannelPaths::live()?)
}

#[tauri::command]
pub fn switch_codex_channel(target: ChannelTarget) -> Result<ChannelSwitchResult, String> {
    let _guard = CHANNEL_FILES_LOCK.lock();
    switch_at(&ChannelPaths::live()?, target, CommitMode::Normal)
}

#[derive(Clone, Copy)]
struct InstalledApp {
    name: &'static str,
    path: &'static str,
    process_pattern: &'static str,
}

fn installed_codex_app() -> Result<InstalledApp, String> {
    let candidates = [
        InstalledApp {
            name: "Codex",
            path: "/Applications/Codex.app",
            process_pattern: "Codex.app/Contents/MacOS/Codex",
        },
        InstalledApp {
            name: "ChatGPT",
            path: "/Applications/ChatGPT.app",
            process_pattern: "ChatGPT.app/Contents/MacOS/ChatGPT",
        },
    ];
    candidates
        .iter()
        .copied()
        .find(|app| process_running(app.process_pattern))
        .or_else(|| {
            candidates
                .into_iter()
                .find(|app| Path::new(app.path).is_dir())
        })
        .ok_or_else(|| "未在 /Applications 找到 Codex.app 或 ChatGPT.app".to_string())
}

fn process_running(pattern: &str) -> bool {
    Command::new("/usr/bin/pgrep")
        .args(["-f", pattern])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[tauri::command]
pub fn restart_codex_app() -> Result<CodexRestartResult, String> {
    let app = installed_codex_app()?;
    if process_running(app.process_pattern) {
        let script = format!("tell application \"{}\" to quit", app.name);
        let output = Command::new("/usr/bin/osascript")
            .args(["-e", &script])
            .stdin(Stdio::null())
            .output()
            .map_err(|e| format!("请求退出 {} 失败：{e}", app.name))?;
        if !output.status.success() {
            return Err(format!("{} 拒绝安全退出，请手动保存并退出后重试", app.name));
        }
        for _ in 0..40 {
            if !process_running(app.process_pattern) {
                break;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        if process_running(app.process_pattern) {
            return Err(format!("{} 在 10 秒内未退出；未强制结束进程", app.name));
        }
    }

    let status = Command::new("/usr/bin/open")
        .args(["-a", app.name])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| format!("重新打开 {} 失败：{e}", app.name))?;
    if !status.success() {
        return Err(format!("重新打开 {} 失败（open exit {status}）", app.name));
    }
    Ok(CodexRestartResult {
        application: app.name.into(),
        reopened: true,
        message: format!("{} 已安全退出并重新打开", app.name),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempCodexDir(PathBuf);

    impl TempCodexDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "provider-hub-channel-switch-{}",
                Uuid::new_v4().simple()
            ));
            fs::create_dir_all(path.join("model-catalogs")).unwrap();
            Self(path)
        }

        fn paths(&self) -> ChannelPaths {
            ChannelPaths::for_dir(&self.0)
        }
    }

    impl Drop for TempCodexDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn fixture() -> (TempCodexDir, ChannelPaths, Vec<u8>, Vec<u8>) {
        let dir = TempCodexDir::new();
        let paths = dir.paths();
        let auth = br#"{
  "auth_mode": "apikey",
  "OPENAI_API_KEY": "fixture-gateway-key",
  "unrelated": "preserve-me",
  "tokens": {
    "access_token": "fixture-access-token",
    "refresh_token": "fixture-refresh-token"
  }
}
"#
        .to_vec();
        let catalog = dir.0.join("model-catalogs/aihub-sub2api.json");
        fs::write(&catalog, b"{\"models\":[]}").unwrap();
        let config = format!(
            r#"model = "sub2api-gpt-5.6-sol"
model_provider = "sub2api"
preferred_auth_method = "apikey"
approval_policy = "never"
openai_base_url = "http://127.0.0.1:18080/v1"
model_catalog_json = "{}"

[model_providers.sub2api]
base_url = "http://127.0.0.1:18080/v1"
wire_api = "responses"
"#,
            catalog.display()
        )
        .into_bytes();
        fs::write(&paths.auth, &auth).unwrap();
        fs::write(&paths.config, &config).unwrap();
        (dir, paths, auth, config)
    }

    fn backup_count(dir: &Path, needle: &str) -> usize {
        fs::read_dir(dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().contains(needle))
            .count()
    }

    #[test]
    fn switch_creates_purpose_timestamped_backups() {
        let (dir, paths, old_auth, old_config) = fixture();
        let result = switch_at(&paths, ChannelTarget::Official, CommitMode::Normal).unwrap();
        assert!(Path::new(&result.auth_backup_path).is_file());
        assert!(Path::new(&result.config_backup_path).is_file());
        assert_eq!(fs::read(&result.auth_backup_path).unwrap(), old_auth);
        assert_eq!(fs::read(&result.config_backup_path).unwrap(), old_config);
        assert!(result.auth_backup_path.contains(".bak-switch-official-"));
        assert!(result.config_backup_path.contains(".bak-switch-official-"));
        assert_eq!(backup_count(&dir.0, ".bak-switch-official-"), 2);
        #[cfg(unix)]
        assert_eq!(
            fs::metadata(&result.auth_backup_path)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    #[test]
    fn official_and_gateway_roundtrip_restores_profiles_without_exposing_secrets() {
        let (_dir, paths, _, _) = fixture();
        let official = switch_at(&paths, ChannelTarget::Official, CommitMode::Normal).unwrap();
        assert_eq!(official.status.current, ActiveChannel::Official);
        assert!(official.status.config_consistent);

        let official_auth = parse_auth(&fs::read(&paths.auth).unwrap()).unwrap();
        let official_config = parse_config(&fs::read(&paths.config).unwrap()).unwrap();
        assert_eq!(json_string(&official_auth, "auth_mode"), "chatgpt");
        assert_eq!(json_string(&official_auth, "unrelated"), "preserve-me");
        assert_eq!(toml_string(&official_config, "model"), "gpt-5.6-sol");
        assert_eq!(toml_string(&official_config, "model_provider"), "openai");
        assert!(official_config.get("openai_base_url").is_none());
        assert!(official_config.get("model_catalog_json").is_none());
        assert_eq!(toml_string(&official_config, "approval_policy"), "never");

        let gateway = switch_at(&paths, ChannelTarget::Sub2Api, CommitMode::Normal).unwrap();
        assert_eq!(gateway.status.current, ActiveChannel::Sub2Api);
        assert!(gateway.status.config_consistent);
        assert!(gateway.status.official_profile_saved);
        assert!(gateway.status.sub2api_profile_saved);

        let gateway_auth = parse_auth(&fs::read(&paths.auth).unwrap()).unwrap();
        let gateway_config = parse_config(&fs::read(&paths.config).unwrap()).unwrap();
        assert_eq!(json_string(&gateway_auth, "auth_mode"), "apikey");
        assert_eq!(toml_string(&gateway_config, "model"), "sub2api-gpt-5.6-sol");
        assert_eq!(toml_string(&gateway_config, "model_provider"), "sub2api");
        assert_eq!(
            toml_string(&gateway_config, "openai_base_url"),
            "http://127.0.0.1:18080/v1"
        );
        assert!(!gateway.message.contains("fixture-gateway-key"));
        let profile_text = fs::read_to_string(&paths.profiles).unwrap();
        assert!(!profile_text.contains("fixture-gateway-key"));
        assert!(!profile_text.contains("fixture-access-token"));
    }

    #[test]
    fn failure_after_first_commit_rolls_auth_back_exactly() {
        let (_dir, paths, old_auth, old_config) = fixture();
        let error =
            switch_at(&paths, ChannelTarget::Official, CommitMode::FailAfterFirst).unwrap_err();
        assert!(error.contains("已回滚"));
        assert_eq!(fs::read(&paths.auth).unwrap(), old_auth);
        assert_eq!(fs::read(&paths.config).unwrap(), old_config);
        assert!(!paths.profiles.exists());
    }

    #[test]
    fn credential_errors_never_include_credential_values() {
        let (_dir, paths, _, _) = fixture();
        let mut auth = parse_auth(&fs::read(&paths.auth).unwrap()).unwrap();
        auth.as_object_mut().unwrap().remove("tokens");
        fs::write(&paths.auth, serialize_auth(&auth).unwrap()).unwrap();
        let error = switch_at(&paths, ChannelTarget::Official, CommitMode::Normal).unwrap_err();
        assert!(error.contains("OAuth 凭据"));
        assert!(!error.contains("fixture-gateway-key"));
    }

    #[cfg(unix)]
    #[test]
    fn credential_files_must_not_be_symlinks() {
        let (dir, paths, old_auth, _) = fixture();
        let target = dir.0.join("real-auth.json");
        fs::write(&target, old_auth).unwrap();
        fs::remove_file(&paths.auth).unwrap();
        std::os::unix::fs::symlink(&target, &paths.auth).unwrap();

        let error = get_status_at(&paths).unwrap_err();
        assert!(error.contains("符号链接"));
    }

    #[test]
    fn concurrent_auth_refresh_is_never_overwritten() {
        let (_dir, paths, _, old_config) = fixture();
        let error = switch_at(
            &paths,
            ChannelTarget::Official,
            CommitMode::ConcurrentAuthBeforeCommit,
        )
        .unwrap_err();
        assert!(error.contains("并发修改了 auth.json"));
        let current_auth = fs::read_to_string(&paths.auth).unwrap();
        assert!(current_auth.contains("rotated"));
        assert_eq!(fs::read(&paths.config).unwrap(), old_config);
        assert!(!paths.profiles.exists());
    }

    #[test]
    fn concurrent_config_change_rolls_back_our_auth_without_overwriting_config() {
        let (_dir, paths, old_auth, _) = fixture();
        let error = switch_at(
            &paths,
            ChannelTarget::Official,
            CommitMode::ConcurrentConfigAfterFirst,
        )
        .unwrap_err();
        assert!(error.contains("并发修改了 config.toml"));
        assert!(error.contains("auth.json 已自动回滚"));
        assert_eq!(fs::read(&paths.auth).unwrap(), old_auth);
        assert_eq!(
            fs::read(&paths.config).unwrap(),
            b"# concurrently changed\n"
        );
        assert!(!paths.profiles.exists());
    }

    #[test]
    fn status_rejects_gateway_prefixed_or_empty_official_models() {
        let (_dir, paths, _, _) = fixture();
        let auth = parse_auth(&fs::read(&paths.auth).unwrap()).unwrap();
        let mut config = parse_config(&fs::read(&paths.config).unwrap()).unwrap();
        set_config_string(&mut config, "model_provider", "openai").unwrap();
        set_config_string(&mut config, "preferred_auth_method", "chatgpt").unwrap();
        remove_config_key(&mut config, "openai_base_url").unwrap();
        remove_config_key(&mut config, "model_catalog_json").unwrap();

        let mut official_auth = auth.clone();
        set_json_string(&mut official_auth, "auth_mode", "chatgpt").unwrap();
        for invalid_model in [
            "",
            "sub2api-gpt-5.6-sol",
            "aihub-gpt-5.6-sol",
            "anyrouter-gpt-5.6-sol",
        ] {
            set_config_string(&mut config, "model", invalid_model).unwrap();
            assert!(
                !status_from_docs(&official_auth, &config, &ChannelProfiles::default())
                    .config_consistent
            );
        }
    }

    #[test]
    fn sub2api_requires_a_usable_provider_base_url() {
        let (_dir, paths, _, _) = fixture();
        let mut config = parse_config(&fs::read(&paths.config).unwrap()).unwrap();
        config_table_mut(&mut config)
            .unwrap()
            .remove("model_providers");
        fs::write(&paths.config, serialize_config(&config).unwrap()).unwrap();

        let error = switch_at(&paths, ChannelTarget::Sub2Api, CommitMode::Normal).unwrap_err();
        assert!(error.contains("model_providers.sub2api"));
        assert!(
            !status_from_docs(
                &parse_auth(&fs::read(&paths.auth).unwrap()).unwrap(),
                &config,
                &ChannelProfiles::default(),
            )
            .config_consistent
        );
    }
}
