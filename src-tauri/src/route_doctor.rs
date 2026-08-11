//! Read-only route diagnosis and guarded Sub2API repair primitives.
//!
//! Integration contract:
//! - Resolve the current gateway API-key **id** in `sub2api.rs`; never pass the
//!   raw key into this module or a child-process argument.
//! - Call [`DockerRouteDoctor::load_snapshot`] and [`diagnose`] for the normal
//!   (read-only) button path.
//! - Relay `/v1/models` probes are read-only. A minimal `/v1/responses` probe is
//!   deliberately opt-in because it consumes upstream quota.
//! - Applying a repair requires an explicit confirmation phrase. The executor
//!   backs up Postgres, writes local + Sub2API audit records, performs only the
//!   SQL compiled from [`RepairAction`], and restarts only the app container.

use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;
use uuid::Uuid;

const DEFAULT_POSTGRES_CONTAINER: &str = "sub2api-json-proxy-postgres";
const DEFAULT_APP_CONTAINER: &str = "sub2api-json-proxy-app";
const APPLY_CONFIRMATION: &str = "APPLY_ROUTE_DOCTOR_REPAIR";
const MAX_CAPTURE_BYTES: usize = 8 * 1024 * 1024;
const PROBE_TIMEOUT: Duration = Duration::from_secs(12);

static REPAIR_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ModelContext {
    pub client_model: String,
    pub expected_upstream_model: String,
}

impl ModelContext {
    pub fn new(
        client_model: impl Into<String>,
        expected: impl Into<String>,
    ) -> Result<Self, String> {
        let client_model = validate_model_name(&client_model.into(), "client model")?;
        let expected_upstream_model = validate_model_name(&expected.into(), "upstream model")?;
        Ok(Self {
            client_model,
            expected_upstream_model,
        })
    }

    /// Derive the likely upstream model from a Hub-prefixed catalog slug.
    pub fn from_client_model(client_model: impl Into<String>) -> Result<Self, String> {
        let client_model = validate_model_name(&client_model.into(), "client model")?;
        let expected = strip_provider_prefix(&client_model).unwrap_or_else(|| client_model.clone());
        Self::new(client_model, expected)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ApiKeySnapshot {
    pub id: i64,
    pub name: String,
    pub status: String,
    pub group_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GroupSnapshot {
    pub id: i64,
    pub name: String,
    pub status: String,
    pub platform: String,
    pub require_oauth_only: bool,
    pub fallback_group_id: Option<i64>,
    #[serde(default)]
    pub supported_model_scopes: Vec<String>,
    /// True only for the JSONB literal `null`; SQL NULL is tracked separately.
    pub supported_model_scopes_json_null: bool,
    pub supported_model_scopes_sql_null: bool,
    pub supported_model_scopes_invalid: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AccountSnapshot {
    pub id: i64,
    pub name: String,
    pub account_type: String,
    pub status: String,
    pub schedulable: bool,
    pub rate_limit_reset_at: Option<DateTime<Utc>>,
    pub overload_until: Option<DateTime<Utc>>,
    pub temp_unschedulable_until: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing)]
    pub model_mapping: Value,
    #[serde(default, skip_serializing)]
    pub model_rate_limits: Value,
}

impl AccountSnapshot {
    pub fn is_relay(&self) -> bool {
        matches!(self.account_type.as_str(), "apikey" | "api_key")
    }

    pub fn is_oauth(&self) -> bool {
        self.account_type == "oauth"
    }

    pub fn mapped_model(&self, requested: &str) -> Option<String> {
        resolve_model_mapping(&self.model_mapping, requested)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MembershipSnapshot {
    pub account_id: i64,
    pub group_id: i64,
    pub priority: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DoctorSnapshot {
    pub api_key: Option<ApiKeySnapshot>,
    pub groups: Vec<GroupSnapshot>,
    pub accounts: Vec<AccountSnapshot>,
    pub memberships: Vec<MembershipSnapshot>,
    pub model: ModelContext,
    pub captured_at: DateTime<Utc>,
}

impl DoctorSnapshot {
    fn group(&self, id: i64) -> Option<&GroupSnapshot> {
        self.groups.iter().find(|group| group.id == id)
    }

    fn account(&self, id: i64) -> Option<&AccountSnapshot> {
        self.accounts.iter().find(|account| account.id == id)
    }

    fn member_accounts(&self, group_id: i64) -> Vec<&AccountSnapshot> {
        self.memberships
            .iter()
            .filter(|membership| membership.group_id == group_id)
            .filter_map(|membership| self.account(membership.account_id))
            .collect()
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
pub enum IssueSeverity {
    Info,
    Warning,
    Critical,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosisIssue {
    pub code: String,
    pub severity: IssueSeverity,
    pub title: String,
    pub detail: String,
    pub account_ids: Vec<i64>,
    pub group_id: Option<i64>,
    pub repair: Option<RepairAction>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosisReport {
    pub healthy: bool,
    pub current_api_key_id: Option<i64>,
    pub current_group_id: Option<i64>,
    pub current_group_name: Option<String>,
    pub current_model: String,
    pub usable_member_count: usize,
    pub issues: Vec<DiagnosisIssue>,
    pub generated_at: DateTime<Utc>,
}

/// Closed repair vocabulary. No caller-supplied SQL is ever accepted.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(
    tag = "action",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum RepairAction {
    SetAccountsSchedulable {
        account_ids: Vec<i64>,
    },
    ClearModelRateLimits {
        account_ids: Vec<i64>,
    },
    ResetTransientParking {
        account_ids: Vec<i64>,
    },
    SetGroupSupportedScopes {
        group_id: i64,
        scopes: Vec<String>,
    },
    SetGroupRequireOauthOnly {
        group_id: i64,
        enabled: bool,
    },
    AddModelMapping {
        account_ids: Vec<i64>,
        client_model: String,
        upstream_model: String,
    },
    EnsureRelayFallback {
        group_id: i64,
        relay_account_ids: Vec<i64>,
        priority: i32,
    },
    MoveApiKeyToGroup {
        api_key_id: i64,
        target_group_id: i64,
    },
    SetApiKeyActive {
        api_key_id: i64,
    },
    SetGroupActive {
        group_id: i64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PlannedChange {
    pub entity: String,
    pub entity_id: i64,
    pub field: String,
    pub old_value: Value,
    pub new_value: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RepairPlan {
    pub action: RepairAction,
    pub summary: String,
    pub changes: Vec<PlannedChange>,
    pub backup_required: bool,
    pub app_restart_required: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum RepairMode {
    DryRun,
    Apply,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RepairRequest {
    pub action: RepairAction,
    pub mode: RepairMode,
    pub actor: String,
    pub confirmation: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RepairResult {
    pub plan: RepairPlan,
    pub applied: bool,
    pub backup_path: Option<String>,
    pub request_id: Option<String>,
    pub message: String,
}

#[derive(Debug)]
struct CompiledRepair {
    plan: RepairPlan,
    mutation_sql: String,
}

fn validate_id(id: i64, label: &str) -> Result<i64, String> {
    if id <= 0 {
        Err(format!("{label} must be positive"))
    } else {
        Ok(id)
    }
}

fn normalize_ids(ids: &[i64], label: &str) -> Result<Vec<i64>, String> {
    if ids.is_empty() {
        return Err(format!("{label} cannot be empty"));
    }
    let mut unique = BTreeSet::new();
    for id in ids {
        unique.insert(validate_id(*id, label)?);
    }
    if unique.len() > 100 {
        return Err(format!("{label} exceeds 100 entries"));
    }
    Ok(unique.into_iter().collect())
}

fn validate_model_name(raw: &str, label: &str) -> Result<String, String> {
    let value = raw.trim();
    if value.is_empty() || value.len() > 160 || value.chars().any(char::is_control) {
        return Err(format!("invalid {label}"));
    }
    Ok(value.to_string())
}

fn validate_actor(raw: &str) -> Result<String, String> {
    let value = raw.trim();
    if value.is_empty() || value.len() > 120 || value.chars().any(char::is_control) {
        return Err("invalid repair actor".into());
    }
    Ok(value.to_string())
}

fn strip_provider_prefix(slug: &str) -> Option<String> {
    let (_, remainder) = slug.split_once('-')?;
    let recognized = ["gpt", "codex", "claude", "gemini", "grok", "o1", "o3"]
        .iter()
        .any(|family| remainder.starts_with(family) || remainder.contains(&format!("-{family}")));
    recognized.then(|| remainder.to_string())
}

fn wildcard_matches(pattern: &str, value: &str) -> bool {
    if !pattern.contains('*') {
        return pattern == value;
    }
    let mut remainder = value;
    let mut first = true;
    for part in pattern.split('*') {
        if part.is_empty() {
            first = false;
            continue;
        }
        if first && !pattern.starts_with('*') {
            let Some(next) = remainder.strip_prefix(part) else {
                return false;
            };
            remainder = next;
        } else if let Some(index) = remainder.find(part) {
            remainder = &remainder[index + part.len()..];
        } else {
            return false;
        }
        first = false;
    }
    pattern.ends_with('*') || remainder.is_empty()
}

fn resolve_model_mapping(mapping: &Value, requested: &str) -> Option<String> {
    let object = mapping.as_object()?;
    if let Some(mapped) = object.get(requested).and_then(Value::as_str) {
        return Some(mapped.trim().to_string()).filter(|value| !value.is_empty());
    }
    object
        .iter()
        .filter(|(pattern, value)| pattern.contains('*') && value.is_string())
        .filter(|(pattern, _)| wildcard_matches(pattern, requested))
        .max_by_key(|(pattern, _)| pattern.chars().filter(|ch| *ch != '*').count())
        .and_then(|(_, value)| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn timestamp_from_value(value: &Value) -> Option<DateTime<Utc>> {
    let raw = value
        .get("rate_limit_reset_at")
        .or_else(|| value.get("reset_at"))
        .unwrap_or(value);
    if let Some(seconds) = raw.as_i64() {
        let seconds = if seconds > 10_000_000_000 {
            seconds / 1_000
        } else {
            seconds
        };
        return DateTime::from_timestamp(seconds, 0);
    }
    raw.as_str()
        .and_then(|text| DateTime::parse_from_rfc3339(text).ok())
        .map(|timestamp| timestamp.with_timezone(&Utc))
}

fn model_parking_until(account: &AccountSnapshot, model: &ModelContext) -> Option<DateTime<Utc>> {
    let mapped = account
        .mapped_model(&model.client_model)
        .unwrap_or_else(|| model.expected_upstream_model.clone());
    account
        .model_rate_limits
        .as_object()
        .into_iter()
        .flat_map(|limits| limits.iter())
        .filter(|(key, _)| *key == &model.client_model || *key == &mapped)
        .filter_map(|(_, value)| timestamp_from_value(value))
        .max()
}

fn transient_parking_until(
    account: &AccountSnapshot,
    model: &ModelContext,
) -> Option<DateTime<Utc>> {
    [
        account.rate_limit_reset_at,
        account.overload_until,
        account.temp_unschedulable_until,
        model_parking_until(account, model),
    ]
    .into_iter()
    .flatten()
    .max()
}

fn account_usable(
    account: &AccountSnapshot,
    group: &GroupSnapshot,
    model: &ModelContext,
    now: DateTime<Utc>,
) -> bool {
    if account.status != "active" || !account.schedulable {
        return false;
    }
    if group.require_oauth_only && !account.is_oauth() {
        return false;
    }
    if transient_parking_until(account, model).is_some_and(|until| until > now) {
        return false;
    }
    // A prefixed catalog model must be explicitly mapped. Sending it verbatim
    // upstream is the known 400 -> parking -> 503 failure chain.
    if strip_provider_prefix(&model.client_model).is_some()
        && account.mapped_model(&model.client_model).is_none()
    {
        return false;
    }
    true
}

fn usable_accounts<'a>(
    snapshot: &'a DoctorSnapshot,
    group: &'a GroupSnapshot,
    now: DateTime<Utc>,
) -> Vec<&'a AccountSnapshot> {
    if group.status != "active" {
        return Vec::new();
    }
    snapshot
        .member_accounts(group.id)
        .into_iter()
        .filter(|account| account_usable(account, group, &snapshot.model, now))
        .collect()
}

fn best_usable_group(
    snapshot: &DoctorSnapshot,
    now: DateTime<Utc>,
    exclude: Option<i64>,
) -> Option<i64> {
    snapshot
        .groups
        .iter()
        .filter(|group| Some(group.id) != exclude)
        .filter(|group| group.status == "active" && group.platform == "openai")
        .map(|group| (group.id, usable_accounts(snapshot, group, now).len()))
        .filter(|(_, usable)| *usable > 0)
        .max_by_key(|(id, usable)| (*usable, std::cmp::Reverse(*id)))
        .map(|(id, _)| id)
}

fn issue(
    code: &str,
    severity: IssueSeverity,
    title: impl Into<String>,
    detail: impl Into<String>,
    account_ids: Vec<i64>,
    group_id: Option<i64>,
    repair: Option<RepairAction>,
) -> DiagnosisIssue {
    DiagnosisIssue {
        code: code.into(),
        severity,
        title: title.into(),
        detail: detail.into(),
        account_ids,
        group_id,
        repair,
    }
}

/// Diagnose a normalized, credential-free snapshot. This function never does I/O.
pub fn diagnose(snapshot: &DoctorSnapshot, now: DateTime<Utc>) -> DiagnosisReport {
    let mut issues = Vec::new();
    let mut current_group = None;
    let mut usable_member_count = 0;

    let Some(api_key) = snapshot.api_key.as_ref() else {
        issues.push(issue(
            "api_key_not_found",
            IssueSeverity::Critical,
            "当前 API key 不存在",
            "Hub 未能按安全的 key id 找到当前网关 key；未读取或显示原始 key。",
            vec![],
            None,
            None,
        ));
        return DiagnosisReport {
            healthy: false,
            current_api_key_id: None,
            current_group_id: None,
            current_group_name: None,
            current_model: snapshot.model.client_model.clone(),
            usable_member_count,
            issues,
            generated_at: now,
        };
    };

    if api_key.status != "active" {
        issues.push(issue(
            "api_key_inactive",
            IssueSeverity::Critical,
            "当前 API key 未启用",
            format!("API key #{} 的状态为 {}。", api_key.id, api_key.status),
            vec![],
            api_key.group_id,
            Some(RepairAction::SetApiKeyActive {
                api_key_id: api_key.id,
            }),
        ));
    }

    match api_key.group_id {
        None => {
            let target = best_usable_group(snapshot, now, None);
            issues.push(issue(
                "api_key_group_missing",
                IssueSeverity::Critical,
                "当前 API key 没有分组",
                "没有 group_id 时调度边界不明确，无法保证号池与中转兜底。",
                vec![],
                None,
                target.map(|target_group_id| RepairAction::MoveApiKeyToGroup {
                    api_key_id: api_key.id,
                    target_group_id,
                }),
            ));
        }
        Some(group_id) => match snapshot.group(group_id) {
            None => {
                let target = best_usable_group(snapshot, now, Some(group_id));
                issues.push(issue(
                    "api_key_group_not_found",
                    IssueSeverity::Critical,
                    "当前 API key 指向不存在的分组",
                    format!(
                        "API key #{} 指向 group #{group_id}，但该组不存在或已删除。",
                        api_key.id
                    ),
                    vec![],
                    Some(group_id),
                    target.map(|target_group_id| RepairAction::MoveApiKeyToGroup {
                        api_key_id: api_key.id,
                        target_group_id,
                    }),
                ));
            }
            Some(group) => {
                current_group = Some(group);
                if group.status != "active" {
                    issues.push(issue(
                        "group_inactive",
                        IssueSeverity::Critical,
                        "当前分组未启用",
                        format!("分组 #{} 的状态为 {}。", group.id, group.status),
                        vec![],
                        Some(group.id),
                        Some(RepairAction::SetGroupActive { group_id: group.id }),
                    ));
                }

                if group.supported_model_scopes_json_null {
                    issues.push(issue(
                        "supported_model_scopes_json_null",
                        IssueSeverity::Critical,
                        "supported_model_scopes 是 JSON null",
                        "这是 JSONB 字面量 null（::text='null'），不是 SQL NULL；调度器会把整组排除。空数组表示不限制。",
                        vec![],
                        Some(group.id),
                        Some(RepairAction::SetGroupSupportedScopes {
                            group_id: group.id,
                            scopes: vec![],
                        }),
                    ));
                } else if group.supported_model_scopes_sql_null
                    || group.supported_model_scopes_invalid
                {
                    issues.push(issue(
                        "supported_model_scopes_invalid",
                        IssueSeverity::Critical,
                        "supported_model_scopes 无效",
                        "字段不是有效数组；修复为空数组可恢复“不限制模型范围”的语义。",
                        vec![],
                        Some(group.id),
                        Some(RepairAction::SetGroupSupportedScopes {
                            group_id: group.id,
                            scopes: vec![],
                        }),
                    ));
                } else if !group.supported_model_scopes.is_empty()
                    && !group
                        .supported_model_scopes
                        .iter()
                        .any(|scope| scope == "openai")
                {
                    let mut scopes = group.supported_model_scopes.clone();
                    scopes.push("openai".into());
                    scopes.sort();
                    scopes.dedup();
                    issues.push(issue(
                        "supported_model_scopes_excludes_openai",
                        IssueSeverity::Critical,
                        "分组模型范围不含 OpenAI",
                        "当前是 OpenAI 路由，但 supported_model_scopes 没有 openai。",
                        vec![],
                        Some(group.id),
                        Some(RepairAction::SetGroupSupportedScopes {
                            group_id: group.id,
                            scopes,
                        }),
                    ));
                }

                let members = snapshot.member_accounts(group.id);
                let oauth_ids: Vec<i64> = members
                    .iter()
                    .filter(|account| account.is_oauth())
                    .map(|account| account.id)
                    .collect();
                let relay_ids: Vec<i64> = members
                    .iter()
                    .filter(|account| account.is_relay())
                    .map(|account| account.id)
                    .collect();

                if members.is_empty() {
                    issues.push(issue(
                        "group_has_no_members",
                        IssueSeverity::Critical,
                        "当前分组没有成员",
                        "account_groups 中没有任何有效成员。fallback_group_id 不能修复组内空池。",
                        vec![],
                        Some(group.id),
                        best_usable_group(snapshot, now, Some(group.id)).map(|target_group_id| {
                            RepairAction::MoveApiKeyToGroup {
                                api_key_id: api_key.id,
                                target_group_id,
                            }
                        }),
                    ));
                }

                if group.require_oauth_only && oauth_ids.is_empty() && !relay_ids.is_empty() {
                    issues.push(issue(
                        "oauth_only_excludes_all_members",
                        IssueSeverity::Critical,
                        "OAuth-only 规则排除了全部成员",
                        "组里只有 apikey 中转账号，但 require_oauth_only=true。",
                        relay_ids.clone(),
                        Some(group.id),
                        Some(RepairAction::SetGroupRequireOauthOnly {
                            group_id: group.id,
                            enabled: false,
                        }),
                    ));
                }

                let all_relay_candidates: Vec<i64> = snapshot
                    .accounts
                    .iter()
                    .filter(|account| {
                        account.is_relay()
                            && account.status == "active"
                            && account.schedulable
                            && transient_parking_until(account, &snapshot.model)
                                .is_none_or(|until| until <= now)
                    })
                    .map(|account| account.id)
                    .collect();
                if relay_ids.is_empty() {
                    let repair = (!all_relay_candidates.is_empty()).then(|| {
                        RepairAction::EnsureRelayFallback {
                            group_id: group.id,
                            relay_account_ids: all_relay_candidates.clone(),
                            // In v0.1.173 account_groups.priority is an
                            // ascending membership order; accounts.priority
                            // controls OpenAI scheduling. Keep the relay late
                            // in group order while preserving it as fallback.
                            priority: 100,
                        }
                    });
                    issues.push(issue(
                        "relay_fallback_missing",
                        IssueSeverity::Warning,
                        "当前组没有中转兜底",
                        "OAuth 的真实额度耗尽时，fallback_group_id 不会因“组内无可用账号”接管；中转账号必须保留在当前组且允许调度。",
                        all_relay_candidates,
                        Some(group.id),
                        repair,
                    ));
                } else if group.require_oauth_only {
                    issues.push(issue(
                        "relay_fallback_filtered",
                        IssueSeverity::Critical,
                        "中转兜底被 OAuth-only 规则过滤",
                        "中转成员虽然在组里，但 require_oauth_only=true 会在 OAuth 全灭时继续返回 503。",
                        relay_ids.clone(),
                        Some(group.id),
                        Some(RepairAction::SetGroupRequireOauthOnly {
                            group_id: group.id,
                            enabled: false,
                        }),
                    ));
                }

                let disabled: Vec<i64> = members
                    .iter()
                    .filter(|account| !account.schedulable)
                    .map(|account| account.id)
                    .collect();
                if !disabled.is_empty() {
                    issues.push(issue(
                        "members_not_schedulable",
                        if disabled.len() == members.len() {
                            IssueSeverity::Critical
                        } else {
                            IssueSeverity::Warning
                        },
                        "组内账号被禁止调度",
                        format!("{} 个成员 schedulable=false。", disabled.len()),
                        disabled.clone(),
                        Some(group.id),
                        Some(RepairAction::SetAccountsSchedulable {
                            account_ids: disabled,
                        }),
                    ));
                }

                let parked: Vec<i64> = members
                    .iter()
                    .filter(|account| {
                        transient_parking_until(account, &snapshot.model)
                            .is_some_and(|until| until > now)
                    })
                    .map(|account| account.id)
                    .collect();
                if !parked.is_empty() {
                    let model_only = parked.iter().all(|account_id| {
                        snapshot.account(*account_id).is_some_and(|account| {
                            model_parking_until(account, &snapshot.model)
                                .is_some_and(|until| until > now)
                                && [
                                    account.rate_limit_reset_at,
                                    account.overload_until,
                                    account.temp_unschedulable_until,
                                ]
                                .into_iter()
                                .flatten()
                                .all(|until| until <= now)
                        })
                    });
                    issues.push(issue(
                        "members_parked",
                        if parked.len() == members.len() {
                            IssueSeverity::Critical
                        } else {
                            IssueSeverity::Warning
                        },
                        "当前模型存在停车账号",
                        "账号处于 rate limit / overload / temp unschedulable / model_rate_limits 窗口。清除停车只适合确认是错误模型名导致的误停车；真实额度耗尽应等待重置并让中转兜底。",
                        parked.clone(),
                        Some(group.id),
                        Some(if model_only {
                            RepairAction::ClearModelRateLimits {
                                account_ids: parked,
                            }
                        } else {
                            RepairAction::ResetTransientParking {
                                account_ids: parked,
                            }
                        }),
                    ));
                }

                let missing_mapping: Vec<i64> = members
                    .iter()
                    .filter(|account| account.mapped_model(&snapshot.model.client_model).is_none())
                    .map(|account| account.id)
                    .collect();
                if !missing_mapping.is_empty() {
                    issues.push(issue(
                        "current_model_mapping_missing",
                        if strip_provider_prefix(&snapshot.model.client_model).is_some() {
                            IssueSeverity::Critical
                        } else {
                            IssueSeverity::Warning
                        },
                        "当前模型缺少账号级映射",
                        format!(
                            "{} 个成员没有 {} → {} 的 model_mapping；带前缀名原样上游会触发 400 并停车。",
                            missing_mapping.len(),
                            snapshot.model.client_model,
                            snapshot.model.expected_upstream_model
                        ),
                        missing_mapping.clone(),
                        Some(group.id),
                        Some(RepairAction::AddModelMapping {
                            account_ids: missing_mapping,
                            client_model: snapshot.model.client_model.clone(),
                            upstream_model: snapshot.model.expected_upstream_model.clone(),
                        }),
                    ));
                }

                usable_member_count = usable_accounts(snapshot, group, now).len();
                if usable_member_count == 0 && !members.is_empty() {
                    let target = best_usable_group(snapshot, now, Some(group.id));
                    issues.push(issue(
                        "group_has_no_usable_accounts",
                        IssueSeverity::Critical,
                        "当前分组没有可用账号",
                        "综合账号状态、schedulable、停车、OAuth-only 与当前模型映射后，可用成员为 0；这会产生 no available accounts / 503。",
                        members.iter().map(|account| account.id).collect(),
                        Some(group.id),
                        target.map(|target_group_id| RepairAction::MoveApiKeyToGroup {
                            api_key_id: api_key.id,
                            target_group_id,
                        }),
                    ));
                }
            }
        },
    }

    issues.sort_by(|left, right| right.severity.cmp(&left.severity));
    DiagnosisReport {
        healthy: !issues
            .iter()
            .any(|finding| finding.severity == IssueSeverity::Critical),
        current_api_key_id: Some(api_key.id),
        current_group_id: api_key.group_id,
        current_group_name: current_group.map(|group| group.name.clone()),
        current_model: snapshot.model.client_model.clone(),
        usable_member_count,
        issues,
        generated_at: now,
    }
}

fn sql_string(raw: &str) -> Result<String, String> {
    if raw.contains('\0') {
        return Err("SQL value contains NUL".into());
    }
    Ok(format!("'{}'", raw.replace('\'', "''")))
}

fn jsonb_sql(value: &Value) -> Result<String, String> {
    let encoded = serde_json::to_string(value).map_err(|error| format!("encode JSON: {error}"))?;
    Ok(format!("{}::jsonb", sql_string(&encoded)?))
}

fn id_list_sql(ids: &[i64]) -> String {
    ids.iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

fn planned_change(
    entity: &str,
    entity_id: i64,
    field: &str,
    old_value: Value,
    new_value: Value,
) -> PlannedChange {
    PlannedChange {
        entity: entity.into(),
        entity_id,
        field: field.into(),
        old_value,
        new_value,
    }
}

fn existing_accounts<'a>(
    snapshot: &'a DoctorSnapshot,
    raw_ids: &[i64],
) -> Result<(Vec<i64>, Vec<&'a AccountSnapshot>), String> {
    let ids = normalize_ids(raw_ids, "account id")?;
    let mut accounts = Vec::with_capacity(ids.len());
    for id in &ids {
        accounts.push(
            snapshot
                .account(*id)
                .ok_or_else(|| format!("account #{id} is absent from the fresh snapshot"))?,
        );
    }
    Ok((ids, accounts))
}

fn validate_scopes(scopes: &[String]) -> Result<Vec<String>, String> {
    if scopes.len() > 16 {
        return Err("too many supported model scopes".into());
    }
    let mut normalized = BTreeSet::new();
    for scope in scopes {
        let value = scope.trim();
        if value.is_empty()
            || value.len() > 64
            || !value
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-'))
        {
            return Err("invalid supported model scope".into());
        }
        normalized.insert(value.to_string());
    }
    Ok(normalized.into_iter().collect())
}

fn compile_repair(
    snapshot: &DoctorSnapshot,
    action: &RepairAction,
    now: DateTime<Utc>,
) -> Result<CompiledRepair, String> {
    let (summary, changes, mutation_sql) = match action {
        RepairAction::SetAccountsSchedulable { account_ids } => {
            let (_, accounts) = existing_accounts(snapshot, account_ids)?;
            let ids = accounts
                .iter()
                .filter(|account| !account.schedulable)
                .map(|account| account.id)
                .collect::<Vec<_>>();
            let changes = accounts
                .iter()
                .filter(|account| !account.schedulable)
                .map(|account| {
                    planned_change(
                        "account",
                        account.id,
                        "schedulable",
                        json!(false),
                        json!(true),
                    )
                })
                .collect::<Vec<_>>();
            (
                format!("Enable scheduling for {} account(s)", ids.len()),
                changes,
                format!(
                    "UPDATE accounts SET schedulable = TRUE, updated_at = NOW() \
                     WHERE id IN ({}) AND deleted_at IS NULL;",
                    id_list_sql(&ids)
                ),
            )
        }
        RepairAction::ClearModelRateLimits { account_ids } => {
            let (_, accounts) = existing_accounts(snapshot, account_ids)?;
            let ids = accounts
                .iter()
                .filter(|account| {
                    !account
                        .model_rate_limits
                        .as_object()
                        .is_none_or(|limits| limits.is_empty())
                })
                .map(|account| account.id)
                .collect::<Vec<_>>();
            let changes = accounts
                .iter()
                .filter(|account| ids.contains(&account.id))
                .map(|account| {
                    planned_change(
                        "account",
                        account.id,
                        "extra.model_rate_limits",
                        json!(!account
                            .model_rate_limits
                            .as_object()
                            .is_none_or(|limits| limits.is_empty())),
                        Value::Null,
                    )
                })
                .collect::<Vec<_>>();
            (
                format!("Clear model-specific parking for {} account(s)", ids.len()),
                changes,
                format!(
                    "UPDATE accounts SET extra = (CASE WHEN jsonb_typeof(extra) = 'object' \
                     THEN extra ELSE '{{}}'::jsonb END) - 'model_rate_limits', updated_at = NOW() \
                     WHERE id IN ({}) AND deleted_at IS NULL;",
                    id_list_sql(&ids)
                ),
            )
        }
        RepairAction::ResetTransientParking { account_ids } => {
            let (_, accounts) = existing_accounts(snapshot, account_ids)?;
            let ids = accounts
                .iter()
                .filter(|account| {
                    account.rate_limit_reset_at.is_some()
                        || account.overload_until.is_some()
                        || account.temp_unschedulable_until.is_some()
                        || !account
                            .model_rate_limits
                            .as_object()
                            .is_none_or(|value| value.is_empty())
                })
                .map(|account| account.id)
                .collect::<Vec<_>>();
            let mut changes = Vec::new();
            for account in accounts
                .into_iter()
                .filter(|account| ids.contains(&account.id))
            {
                changes.push(planned_change(
                    "account",
                    account.id,
                    "transient_parking",
                    json!({
                        "rateLimitResetAt": account.rate_limit_reset_at,
                        "overloadUntil": account.overload_until,
                        "tempUnschedulableUntil": account.temp_unschedulable_until,
                        "hasModelRateLimits": !account.model_rate_limits.as_object().is_none_or(|value| value.is_empty()),
                    }),
                    Value::Null,
                ));
            }
            (
                format!("Reset transient parking for {} account(s)", ids.len()),
                changes,
                format!(
                    "UPDATE accounts SET rate_limited_at = NULL, rate_limit_reset_at = NULL, \
                     overload_until = NULL, temp_unschedulable_until = NULL, \
                     temp_unschedulable_reason = NULL, \
                     extra = (CASE WHEN jsonb_typeof(extra) = 'object' THEN extra ELSE '{{}}'::jsonb END) \
                     - 'model_rate_limits', updated_at = NOW() \
                     WHERE id IN ({}) AND deleted_at IS NULL;",
                    id_list_sql(&ids)
                ),
            )
        }
        RepairAction::SetGroupSupportedScopes { group_id, scopes } => {
            let group_id = validate_id(*group_id, "group id")?;
            let group = snapshot
                .group(group_id)
                .ok_or_else(|| format!("group #{group_id} is absent from the fresh snapshot"))?;
            let scopes = validate_scopes(scopes)?;
            let scopes_value = json!(scopes);
            let needs_change = group.supported_model_scopes_json_null
                || group.supported_model_scopes_sql_null
                || group.supported_model_scopes_invalid
                || group.supported_model_scopes != scopes;
            (
                format!("Set supported model scopes for group #{group_id}"),
                needs_change
                    .then(|| {
                        planned_change(
                            "group",
                            group_id,
                            "supported_model_scopes",
                            if group.supported_model_scopes_json_null
                                || group.supported_model_scopes_sql_null
                                || group.supported_model_scopes_invalid
                            {
                                Value::Null
                            } else {
                                json!(group.supported_model_scopes)
                            },
                            scopes_value.clone(),
                        )
                    })
                    .into_iter()
                    .collect(),
                format!(
                    "UPDATE groups SET supported_model_scopes = {}, updated_at = NOW() \
                     WHERE id = {group_id} AND deleted_at IS NULL;",
                    jsonb_sql(&scopes_value)?
                ),
            )
        }
        RepairAction::SetGroupRequireOauthOnly { group_id, enabled } => {
            let group_id = validate_id(*group_id, "group id")?;
            let group = snapshot
                .group(group_id)
                .ok_or_else(|| format!("group #{group_id} is absent from the fresh snapshot"))?;
            (
                format!("Set require_oauth_only={enabled} for group #{group_id}"),
                (group.require_oauth_only != *enabled)
                    .then(|| {
                        planned_change(
                            "group",
                            group_id,
                            "require_oauth_only",
                            json!(group.require_oauth_only),
                            json!(enabled),
                        )
                    })
                    .into_iter()
                    .collect(),
                format!(
                    "UPDATE groups SET require_oauth_only = {}, updated_at = NOW() \
                     WHERE id = {group_id} AND deleted_at IS NULL;",
                    if *enabled { "TRUE" } else { "FALSE" }
                ),
            )
        }
        RepairAction::AddModelMapping {
            account_ids,
            client_model,
            upstream_model,
        } => {
            let (_, accounts) = existing_accounts(snapshot, account_ids)?;
            let client_model = validate_model_name(client_model, "client model")?;
            let upstream_model = validate_model_name(upstream_model, "upstream model")?;
            let affected_ids = accounts
                .iter()
                .filter(|account| {
                    account.mapped_model(&client_model).as_deref() != Some(upstream_model.as_str())
                })
                .map(|account| account.id)
                .collect::<Vec<_>>();
            let changes = accounts
                .iter()
                .filter(|account| affected_ids.contains(&account.id))
                .map(|account| {
                    planned_change(
                        "account",
                        account.id,
                        &format!("credentials.model_mapping.{client_model}"),
                        account
                            .mapped_model(&client_model)
                            .map(Value::String)
                            .unwrap_or(Value::Null),
                        Value::String(upstream_model.clone()),
                    )
                })
                .collect::<Vec<_>>();
            let mapping_patch = json!({ client_model.clone(): upstream_model.clone() });
            (
                format!(
                    "Add model mapping {client_model} -> {upstream_model} to {} account(s)",
                    affected_ids.len()
                ),
                changes,
                format!(
                    "UPDATE accounts SET credentials = jsonb_set(\
                       CASE WHEN jsonb_typeof(credentials) = 'object' THEN credentials ELSE '{{}}'::jsonb END, \
                       '{{model_mapping}}', \
                       (CASE WHEN jsonb_typeof(credentials->'model_mapping') = 'object' \
                         THEN credentials->'model_mapping' ELSE '{{}}'::jsonb END) || {}, TRUE), \
                     updated_at = NOW() WHERE id IN ({}) AND deleted_at IS NULL;",
                    jsonb_sql(&mapping_patch)?,
                    id_list_sql(&affected_ids)
                ),
            )
        }
        RepairAction::EnsureRelayFallback {
            group_id,
            relay_account_ids,
            priority,
        } => {
            let group_id = validate_id(*group_id, "group id")?;
            let group = snapshot
                .group(group_id)
                .ok_or_else(|| format!("group #{group_id} is absent from the fresh snapshot"))?;
            if !(0..=10_000).contains(priority) {
                return Err("fallback priority must be between 0 and 10000".into());
            }
            let (ids, accounts) = existing_accounts(snapshot, relay_account_ids)?;
            if accounts.iter().any(|account| !account.is_relay()) {
                return Err("EnsureRelayFallback accepts apikey accounts only".into());
            }
            let existing: BTreeMap<i64, i32> = snapshot
                .memberships
                .iter()
                .filter(|membership| membership.group_id == group_id)
                .map(|membership| (membership.account_id, membership.priority))
                .collect();
            let mut changes = ids
                .iter()
                .filter(|id| existing.get(id).is_none_or(|current| current > priority))
                .map(|id| {
                    planned_change(
                        "account_group",
                        *id,
                        &format!("group_{group_id}_priority"),
                        existing
                            .get(id)
                            .copied()
                            .map(Value::from)
                            .unwrap_or(Value::Null),
                        json!(priority),
                    )
                })
                .collect::<Vec<_>>();
            if group.require_oauth_only {
                changes.push(planned_change(
                    "group",
                    group_id,
                    "require_oauth_only",
                    json!(true),
                    json!(false),
                ));
            }
            let values = ids
                .iter()
                .map(|account_id| format!("({account_id},{group_id},{priority},NOW())"))
                .collect::<Vec<_>>()
                .join(",");
            (
                format!(
                    "Keep {} relay fallback account(s) in group #{group_id}",
                    ids.len()
                ),
                changes,
                format!(
                    "INSERT INTO account_groups (account_id, group_id, priority, created_at) \
                     VALUES {values} ON CONFLICT (account_id, group_id) DO UPDATE \
                     SET priority = LEAST(account_groups.priority, EXCLUDED.priority); \
                     UPDATE groups SET require_oauth_only = FALSE, updated_at = NOW() \
                     WHERE id = {group_id} AND deleted_at IS NULL;"
                ),
            )
        }
        RepairAction::MoveApiKeyToGroup {
            api_key_id,
            target_group_id,
        } => {
            let api_key_id = validate_id(*api_key_id, "api key id")?;
            let target_group_id = validate_id(*target_group_id, "target group id")?;
            let key = snapshot
                .api_key
                .as_ref()
                .filter(|key| key.id == api_key_id)
                .ok_or_else(|| "repair API key does not match the fresh snapshot".to_string())?;
            let target = snapshot
                .group(target_group_id)
                .ok_or_else(|| format!("target group #{target_group_id} does not exist"))?;
            if target.status != "active" || usable_accounts(snapshot, target, now).is_empty() {
                return Err("refusing to move the key to a group with no usable account".into());
            }
            (
                format!("Move API key #{api_key_id} to usable group #{target_group_id}"),
                (key.group_id != Some(target_group_id))
                    .then(|| {
                        planned_change(
                            "api_key",
                            api_key_id,
                            "group_id",
                            key.group_id.map(Value::from).unwrap_or(Value::Null),
                            json!(target_group_id),
                        )
                    })
                    .into_iter()
                    .collect(),
                format!(
                    "UPDATE api_keys SET group_id = {target_group_id}, updated_at = NOW() \
                     WHERE id = {api_key_id} AND deleted_at IS NULL;"
                ),
            )
        }
        RepairAction::SetApiKeyActive { api_key_id } => {
            let api_key_id = validate_id(*api_key_id, "api key id")?;
            let key = snapshot
                .api_key
                .as_ref()
                .filter(|key| key.id == api_key_id)
                .ok_or_else(|| "repair API key does not match the fresh snapshot".to_string())?;
            (
                format!("Activate API key #{api_key_id}"),
                (key.status != "active")
                    .then(|| {
                        planned_change(
                            "api_key",
                            api_key_id,
                            "status",
                            json!(key.status),
                            json!("active"),
                        )
                    })
                    .into_iter()
                    .collect(),
                format!(
                    "UPDATE api_keys SET status = 'active', updated_at = NOW() \
                     WHERE id = {api_key_id} AND deleted_at IS NULL;"
                ),
            )
        }
        RepairAction::SetGroupActive { group_id } => {
            let group_id = validate_id(*group_id, "group id")?;
            let group = snapshot
                .group(group_id)
                .ok_or_else(|| format!("group #{group_id} is absent from the fresh snapshot"))?;
            (
                format!("Activate group #{group_id}"),
                (group.status != "active")
                    .then(|| {
                        planned_change(
                            "group",
                            group_id,
                            "status",
                            json!(group.status),
                            json!("active"),
                        )
                    })
                    .into_iter()
                    .collect(),
                format!(
                    "UPDATE groups SET status = 'active', updated_at = NOW() \
                     WHERE id = {group_id} AND deleted_at IS NULL;"
                ),
            )
        }
    };

    if changes.is_empty() {
        return Err("repair is already applied; refusing a no-op backup/restart".into());
    }
    Ok(CompiledRepair {
        plan: RepairPlan {
            action: action.clone(),
            summary,
            changes,
            backup_required: true,
            app_restart_required: true,
        },
        mutation_sql,
    })
}

/// Build and validate a repair plan from a fresh snapshot without any I/O.
#[cfg(test)]
pub fn build_repair_plan(
    snapshot: &DoctorSnapshot,
    action: &RepairAction,
    now: DateTime<Utc>,
) -> Result<RepairPlan, String> {
    compile_repair(snapshot, action, now).map(|compiled| compiled.plan)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DatabaseSnapshot {
    api_key: Option<ApiKeySnapshot>,
    #[serde(default)]
    groups: Vec<GroupSnapshot>,
    #[serde(default)]
    accounts: Vec<AccountSnapshot>,
    #[serde(default)]
    memberships: Vec<MembershipSnapshot>,
}

#[derive(Debug, Clone)]
pub struct DockerRouteDoctor {
    docker: PathBuf,
    postgres_container: String,
    app_container: String,
    backup_dir: PathBuf,
    local_audit_path: PathBuf,
}

impl DockerRouteDoctor {
    pub fn new(sub2api_dir: &Path) -> Self {
        let state_dir = sub2api_dir.join("state");
        Self {
            docker: find_docker(),
            postgres_container: DEFAULT_POSTGRES_CONTAINER.into(),
            app_container: DEFAULT_APP_CONTAINER.into(),
            backup_dir: state_dir.join("backups"),
            local_audit_path: state_dir.join("route-doctor-audit.jsonl"),
        }
    }

    #[cfg(test)]
    fn with_paths(
        docker: PathBuf,
        postgres_container: impl Into<String>,
        app_container: impl Into<String>,
        backup_dir: PathBuf,
        local_audit_path: PathBuf,
    ) -> Self {
        Self {
            docker,
            postgres_container: postgres_container.into(),
            app_container: app_container.into(),
            backup_dir,
            local_audit_path,
        }
    }

    /// Load a credential-free relational snapshot in a READ ONLY transaction.
    pub fn load_snapshot(
        &self,
        api_key_id: i64,
        model: ModelContext,
    ) -> Result<DoctorSnapshot, String> {
        let api_key_id = validate_id(api_key_id, "api key id")?;
        let sql = snapshot_sql(api_key_id);
        let raw = self.run_psql(&format!("BEGIN READ ONLY;\n{sql}\nCOMMIT;"))?;
        let payload = raw
            .lines()
            .rev()
            .map(str::trim)
            .find(|line| line.starts_with('{'))
            .ok_or_else(|| "Sub2API snapshot query returned no JSON".to_string())?;
        if payload.len() > MAX_CAPTURE_BYTES {
            return Err("Sub2API snapshot exceeds safe size limit".into());
        }
        let snapshot: DatabaseSnapshot = serde_json::from_str(payload)
            .map_err(|error| format!("parse credential-free route snapshot: {error}"))?;
        Ok(DoctorSnapshot {
            api_key: snapshot.api_key,
            groups: snapshot.groups,
            accounts: snapshot.accounts,
            memberships: snapshot.memberships,
            model,
            captured_at: Utc::now(),
        })
    }

    /// Resolve the DB id in-process. Raw keys are read from psql stdout and
    /// compared in memory; they are never interpolated into SQL/argv, logged,
    /// serialized, or included in errors.
    pub fn resolve_api_key_id(&self, gateway_key: &str) -> Result<i64, String> {
        let gateway_key = gateway_key.trim();
        if gateway_key.is_empty() || gateway_key.len() > 512 {
            return Err("current gateway key is empty or invalid".into());
        }
        #[derive(Deserialize)]
        struct SecretKeyRow {
            id: i64,
            key: String,
        }
        let raw = self.run_psql(
            "BEGIN READ ONLY;\n\
             SELECT COALESCE(jsonb_agg(jsonb_build_object('id', id, 'key', key) ORDER BY id), \
             '[]'::jsonb)::text FROM api_keys WHERE deleted_at IS NULL;\n\
             COMMIT;",
        )?;
        let payload = raw
            .lines()
            .rev()
            .map(str::trim)
            .find(|line| line.starts_with('['))
            .ok_or_else(|| "API key id lookup returned no JSON".to_string())?;
        let rows: Vec<SecretKeyRow> = serde_json::from_str(payload)
            .map_err(|_| "parse protected API key id lookup".to_string())?;
        rows.into_iter()
            .find(|row| row.key == gateway_key)
            .map(|row| row.id)
            .ok_or_else(|| "current gateway API key is not registered in Sub2API".into())
    }

    /// Execute a dry-run or explicitly confirmed repair against a freshly
    /// reloaded snapshot. The raw gateway key is never needed here.
    pub fn repair(
        &self,
        api_key_id: i64,
        model: ModelContext,
        request: RepairRequest,
    ) -> Result<RepairResult, String> {
        let _guard = REPAIR_LOCK.lock();
        let actor = validate_actor(&request.actor)?;
        let snapshot = self.load_snapshot(api_key_id, model.clone())?;
        let compiled = compile_repair(&snapshot, &request.action, Utc::now())?;

        if request.mode == RepairMode::DryRun {
            return Ok(RepairResult {
                plan: compiled.plan,
                applied: false,
                backup_path: None,
                request_id: None,
                message: "Dry-run only: no backup, database write, audit insert, or restart was performed."
                    .into(),
            });
        }
        if request.confirmation.as_deref() != Some(APPLY_CONFIRMATION) {
            return Err(format!(
                "apply mode requires the exact confirmation phrase {APPLY_CONFIRMATION}"
            ));
        }

        let request_id = Uuid::new_v4().to_string();
        let backup_path = self.backup_database(&request_id)?;
        let prepared = LocalAuditRecord {
            timestamp: Utc::now(),
            request_id: request_id.clone(),
            actor: actor.clone(),
            state: "prepared".into(),
            action: compiled.plan.action.clone(),
            summary: compiled.plan.summary.clone(),
            changes: compiled.plan.changes.clone(),
            backup_path: backup_path.display().to_string(),
            error: None,
        };
        // This append must succeed before any database write.
        self.append_local_audit(&prepared)?;

        let db_audit = sub2api_audit_sql(&prepared)?;
        if let Err(error) = self.run_psql(&db_audit) {
            let safe = safe_runtime_error(&error);
            let _ = self.append_local_audit(&prepared.failed(safe.clone()));
            return Err(format!("Sub2API pre-mutation audit failed: {safe}"));
        }

        let mutation = format!("BEGIN;\n{}\nCOMMIT;", compiled.mutation_sql);
        if let Err(error) = self.run_psql(&mutation) {
            let safe = safe_runtime_error(&error);
            let _ = self.append_local_audit(&prepared.failed(safe.clone()));
            return Err(format!(
                "repair mutation failed; backup and pre-audit are retained: {safe}"
            ));
        }

        if let Err(error) = self.restart_app_container() {
            let safe = safe_runtime_error(&error);
            let _ = self.append_local_audit(&prepared.failed(safe.clone()));
            return Err(format!(
                "database repair committed but app-container restart failed; retry restart only: {safe}"
            ));
        }

        let verified = self
            .load_snapshot(api_key_id, model)
            .map(|fresh| action_is_applied(&fresh, &request.action))
            .unwrap_or(false);
        if !verified {
            let message =
                "post-restart verification did not observe the requested state".to_string();
            let _ = self.append_local_audit(&prepared.failed(message.clone()));
            return Err(format!(
                "{message}; do not repeat blindly (backup: {})",
                backup_path.display()
            ));
        }

        self.append_local_audit(&prepared.applied())?;
        Ok(RepairResult {
            plan: compiled.plan,
            applied: true,
            backup_path: Some(backup_path.display().to_string()),
            request_id: Some(request_id),
            message:
                "Repair applied, audited, app container restarted, and database state verified."
                    .into(),
        })
    }

    fn command(&self) -> Command {
        let mut command = Command::new(&self.docker);
        command.env("PATH", augmented_path());
        command
    }

    fn run_psql(&self, sql: &str) -> Result<String, String> {
        let mut child = self
            .command()
            .args([
                "exec",
                "-i",
                &self.postgres_container,
                "sh",
                "-lc",
                "exec psql -X -v ON_ERROR_STOP=1 -U \"${POSTGRES_USER:-postgres}\" -d \"${POSTGRES_DB:-postgres}\" -Atq -f -",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("start local Docker psql: {error}"))?;
        child
            .stdin
            .as_mut()
            .ok_or_else(|| "open psql stdin".to_string())?
            .write_all(sql.as_bytes())
            .map_err(|error| format!("write psql stdin: {error}"))?;
        let output = child
            .wait_with_output()
            .map_err(|error| format!("wait for psql: {error}"))?;
        if !output.status.success() {
            // Never include stdout: credential-reading relay queries may use
            // this same transport. PostgreSQL stderr contains no SQL because
            // psql reads from stdin and echoing is disabled.
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!(
                "local psql exited with {}: {}",
                output.status,
                safe_runtime_error(stderr.trim())
            ));
        }
        if output.stdout.len() > MAX_CAPTURE_BYTES {
            return Err("local psql output exceeds safe size limit".into());
        }
        String::from_utf8(output.stdout).map_err(|_| "local psql returned non-UTF8 output".into())
    }

    fn backup_database(&self, request_id: &str) -> Result<PathBuf, String> {
        fs::create_dir_all(&self.backup_dir)
            .map_err(|error| format!("create route-doctor backup directory: {error}"))?;
        #[cfg(unix)]
        fs::set_permissions(&self.backup_dir, fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("secure route-doctor backup directory: {error}"))?;

        let stamp = Utc::now().format("%Y%m%dT%H%M%SZ");
        let final_path = self
            .backup_dir
            .join(format!("sub2api-route-doctor-{stamp}-{request_id}.dump"));
        let partial_path = self
            .backup_dir
            .join(format!(".route-doctor-{request_id}.partial"));
        let file = secure_create_new(&partial_path)?;

        let output = self
            .command()
            .args([
                "exec",
                "-i",
                &self.postgres_container,
                "sh",
                "-lc",
                "exec pg_dump -U \"${POSTGRES_USER:-postgres}\" -d \"${POSTGRES_DB:-postgres}\" --format=custom --no-owner --no-acl",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::from(file))
            .stderr(Stdio::piped())
            .output()
            .map_err(|error| format!("start pg_dump backup: {error}"))?;
        if !output.status.success() {
            let _ = fs::remove_file(&partial_path);
            return Err(format!(
                "pg_dump backup failed before mutation: {}",
                safe_runtime_error(&String::from_utf8_lossy(&output.stderr))
            ));
        }
        let size = fs::metadata(&partial_path)
            .map_err(|error| format!("inspect pg_dump backup: {error}"))?
            .len();
        if size < 128 {
            let _ = fs::remove_file(&partial_path);
            return Err("pg_dump produced an implausibly small backup; mutation aborted".into());
        }
        fs::rename(&partial_path, &final_path)
            .map_err(|error| format!("finalize pg_dump backup: {error}"))?;
        Ok(final_path)
    }

    fn append_local_audit(&self, record: &LocalAuditRecord) -> Result<(), String> {
        let parent = self
            .local_audit_path
            .parent()
            .ok_or_else(|| "local audit path has no parent".to_string())?;
        fs::create_dir_all(parent)
            .map_err(|error| format!("create Hub audit directory: {error}"))?;
        if self.local_audit_path.exists()
            && fs::symlink_metadata(&self.local_audit_path)
                .map_err(|error| format!("inspect Hub local audit log: {error}"))?
                .file_type()
                .is_symlink()
        {
            return Err("refusing to append Hub audit data through a symlink".into());
        }
        let mut options = OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options
            .open(&self.local_audit_path)
            .map_err(|error| format!("open Hub local audit log: {error}"))?;
        serde_json::to_writer(&mut file, record)
            .map_err(|error| format!("encode Hub audit record: {error}"))?;
        file.write_all(b"\n")
            .and_then(|_| file.sync_data())
            .map_err(|error| format!("persist Hub audit record: {error}"))
    }

    fn restart_app_container(&self) -> Result<(), String> {
        let output = self
            .command()
            .args(["restart", &self.app_container])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .map_err(|error| format!("start app-container restart: {error}"))?;
        if output.status.success() {
            Ok(())
        } else {
            Err(format!(
                "docker restart {} failed: {}",
                self.app_container,
                safe_runtime_error(&String::from_utf8_lossy(&output.stderr))
            ))
        }
    }
}

fn secure_create_new(path: &Path) -> Result<fs::File, String> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    options
        .open(path)
        .map_err(|error| format!("create secure backup {}: {error}", path.display()))
}

fn find_docker() -> PathBuf {
    [
        "/opt/homebrew/bin/docker",
        "/usr/local/bin/docker",
        "/usr/bin/docker",
    ]
    .iter()
    .map(PathBuf::from)
    .find(|path| path.is_file())
    .unwrap_or_else(|| PathBuf::from("docker"))
}

fn augmented_path() -> String {
    let inherited = std::env::var("PATH").unwrap_or_default();
    format!("/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin:{inherited}")
}

fn snapshot_sql(api_key_id: i64) -> String {
    format!(
        r#"
WITH target_key AS (
  SELECT id, name, status, group_id
  FROM api_keys
  WHERE id = {api_key_id} AND deleted_at IS NULL
), openai_groups AS (
  SELECT * FROM groups WHERE platform = 'openai' AND deleted_at IS NULL
), openai_accounts AS (
  SELECT * FROM accounts WHERE platform = 'openai' AND deleted_at IS NULL
)
SELECT jsonb_build_object(
  'apiKey', (SELECT jsonb_build_object(
      'id', id, 'name', name, 'status', status, 'groupId', group_id
    ) FROM target_key),
  'groups', COALESCE((SELECT jsonb_agg(jsonb_build_object(
      'id', g.id,
      'name', g.name,
      'status', g.status,
      'platform', g.platform,
      'requireOauthOnly', g.require_oauth_only,
      'fallbackGroupId', g.fallback_group_id,
      'supportedModelScopes', CASE
        WHEN jsonb_typeof(g.supported_model_scopes) = 'array' THEN g.supported_model_scopes
        ELSE '[]'::jsonb END,
      'supportedModelScopesJsonNull', COALESCE(g.supported_model_scopes::text = 'null', FALSE),
      'supportedModelScopesSqlNull', g.supported_model_scopes IS NULL,
      'supportedModelScopesInvalid', g.supported_model_scopes IS NOT NULL
        AND COALESCE(jsonb_typeof(g.supported_model_scopes), 'null') NOT IN ('array', 'null')
    ) ORDER BY g.id) FROM openai_groups g), '[]'::jsonb),
  'accounts', COALESCE((SELECT jsonb_agg(jsonb_build_object(
      'id', a.id,
      'name', a.name,
      'accountType', a.type,
      'status', a.status,
      'schedulable', a.schedulable,
      'rateLimitResetAt', a.rate_limit_reset_at,
      'overloadUntil', a.overload_until,
      'tempUnschedulableUntil', a.temp_unschedulable_until,
      'modelMapping', CASE WHEN jsonb_typeof(a.credentials->'model_mapping') = 'object'
        THEN a.credentials->'model_mapping' ELSE '{{}}'::jsonb END,
      'modelRateLimits', CASE WHEN jsonb_typeof(a.extra->'model_rate_limits') = 'object'
        THEN a.extra->'model_rate_limits' ELSE '{{}}'::jsonb END
    ) ORDER BY a.id) FROM openai_accounts a), '[]'::jsonb),
  'memberships', COALESCE((SELECT jsonb_agg(jsonb_build_object(
      'accountId', ag.account_id, 'groupId', ag.group_id, 'priority', ag.priority
    ) ORDER BY ag.group_id, ag.priority, ag.account_id)
    FROM account_groups ag
    JOIN openai_groups g ON g.id = ag.group_id
    JOIN openai_accounts a ON a.id = ag.account_id), '[]'::jsonb)
)::text;
"#
    )
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct LocalAuditRecord {
    timestamp: DateTime<Utc>,
    request_id: String,
    actor: String,
    state: String,
    action: RepairAction,
    summary: String,
    changes: Vec<PlannedChange>,
    backup_path: String,
    error: Option<String>,
}

impl LocalAuditRecord {
    fn failed(&self, error: String) -> Self {
        Self {
            timestamp: Utc::now(),
            state: "failed".into(),
            error: Some(safe_runtime_error(&error)),
            ..self.clone()
        }
    }

    fn applied(&self) -> Self {
        Self {
            timestamp: Utc::now(),
            state: "applied".into(),
            error: None,
            ..self.clone()
        }
    }
}

fn sub2api_audit_sql(record: &LocalAuditRecord) -> Result<String, String> {
    let body = serde_json::to_string(record)
        .map_err(|error| format!("encode Sub2API audit payload: {error}"))?;
    let extra = json!({
        "component": "codex-provider-hub",
        "feature": "route-doctor",
        "state": "prepared",
        "requestId": record.request_id,
        "backupPath": record.backup_path,
        "changes": record.changes,
    });
    Ok(format!(
        "INSERT INTO audit_logs (created_at, actor_email, actor_role, auth_method, \
         credential_masked, action, method, path, request_id, client_ip, user_agent, \
         request_body, status_code, latency_ms, extra) VALUES (NOW(), {}, 'local_operator', \
         'hub_local', '', 'route_doctor.repair.prepared', 'LOCAL', '/hub/route-doctor', \
         {}, '127.0.0.1', 'Codex-Provider-Hub/route-doctor', {}, 202, 0, {});",
        sql_string(&record.actor)?,
        sql_string(&record.request_id)?,
        sql_string(&body)?,
        jsonb_sql(&extra)?
    ))
}

fn safe_runtime_error(raw: &str) -> String {
    let normalized = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let lower = normalized.to_ascii_lowercase();
    if [
        "access_token",
        "refresh_token",
        "api_key",
        "authorization:",
        "bearer ",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
    {
        return "operation failed (sensitive upstream detail withheld)".into();
    }
    let value = if normalized.is_empty() {
        "operation failed"
    } else {
        &normalized
    };
    value.chars().take(300).collect()
}

fn action_is_applied(snapshot: &DoctorSnapshot, action: &RepairAction) -> bool {
    match action {
        RepairAction::SetAccountsSchedulable { account_ids } => account_ids.iter().all(|id| {
            snapshot
                .account(*id)
                .is_some_and(|account| account.schedulable)
        }),
        RepairAction::ClearModelRateLimits { account_ids } => account_ids.iter().all(|id| {
            snapshot.account(*id).is_some_and(|account| {
                account
                    .model_rate_limits
                    .as_object()
                    .is_none_or(|limits| limits.is_empty())
            })
        }),
        RepairAction::ResetTransientParking { account_ids } => account_ids.iter().all(|id| {
            snapshot.account(*id).is_some_and(|account| {
                account.rate_limit_reset_at.is_none()
                    && account.overload_until.is_none()
                    && account.temp_unschedulable_until.is_none()
                    && account
                        .model_rate_limits
                        .as_object()
                        .is_none_or(|limits| limits.is_empty())
            })
        }),
        RepairAction::SetGroupSupportedScopes { group_id, scopes } => {
            let Ok(scopes) = validate_scopes(scopes) else {
                return false;
            };
            snapshot.group(*group_id).is_some_and(|group| {
                !group.supported_model_scopes_json_null
                    && !group.supported_model_scopes_sql_null
                    && !group.supported_model_scopes_invalid
                    && group.supported_model_scopes == scopes
            })
        }
        RepairAction::SetGroupRequireOauthOnly { group_id, enabled } => snapshot
            .group(*group_id)
            .is_some_and(|group| group.require_oauth_only == *enabled),
        RepairAction::AddModelMapping {
            account_ids,
            client_model,
            upstream_model,
        } => account_ids.iter().all(|id| {
            snapshot.account(*id).is_some_and(|account| {
                account.mapped_model(client_model).as_deref() == Some(upstream_model.as_str())
            })
        }),
        RepairAction::EnsureRelayFallback {
            group_id,
            relay_account_ids,
            priority,
        } => {
            snapshot
                .group(*group_id)
                .is_some_and(|group| !group.require_oauth_only)
                && relay_account_ids.iter().all(|account_id| {
                    snapshot.memberships.iter().any(|membership| {
                        membership.group_id == *group_id
                            && membership.account_id == *account_id
                            && membership.priority <= *priority
                    })
                })
        }
        RepairAction::MoveApiKeyToGroup {
            api_key_id,
            target_group_id,
        } => snapshot
            .api_key
            .as_ref()
            .is_some_and(|key| key.id == *api_key_id && key.group_id == Some(*target_group_id)),
        RepairAction::SetApiKeyActive { api_key_id } => snapshot
            .api_key
            .as_ref()
            .is_some_and(|key| key.id == *api_key_id && key.status == "active"),
        RepairAction::SetGroupActive { group_id } => snapshot
            .group(*group_id)
            .is_some_and(|group| group.status == "active"),
    }
}

/// Read current model from Codex config without reading auth.json or any token.
pub fn model_context_from_codex_config(config_path: &Path) -> Result<ModelContext, String> {
    let raw = fs::read_to_string(config_path)
        .map_err(|error| format!("read Codex config {}: {error}", config_path.display()))?;
    let document: toml::Value =
        toml::from_str(&raw).map_err(|error| format!("parse Codex config: {error}"))?;
    let model = document
        .get("model")
        .and_then(toml::Value::as_str)
        .ok_or_else(|| "Codex config has no current model".to_string())?;
    ModelContext::from_client_model(model)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawRelayTarget {
    account_id: i64,
    name: String,
    base_url: String,
    api_key: String,
    #[serde(default)]
    model_mapping: Value,
}

/// Ephemeral relay credentials. Intentionally not serializable or cloneable;
/// Debug output is redacted so it cannot leak through logs or Tauri responses.
pub struct RelayProbeTarget {
    account_id: i64,
    name: String,
    base_url: String,
    api_key: String,
    probe_model: String,
}

impl std::fmt::Debug for RelayProbeTarget {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RelayProbeTarget")
            .field("account_id", &self.account_id)
            .field("name", &self.name)
            .field("base_url", &self.base_url)
            .field("api_key", &"[REDACTED]")
            .field("probe_model", &self.probe_model)
            .finish()
    }
}

impl DockerRouteDoctor {
    /// Read relay secrets only when a probe is requested. The query is sent on
    /// stdin, stdout is parsed in memory, and no secret is returned to the UI.
    pub fn load_relay_probe_targets(
        &self,
        account_ids: &[i64],
        model: &ModelContext,
    ) -> Result<Vec<RelayProbeTarget>, String> {
        let ids = normalize_ids(account_ids, "relay account id")?;
        let sql = format!(
            r#"
BEGIN READ ONLY;
SELECT COALESCE(jsonb_agg(jsonb_build_object(
  'accountId', a.id,
  'name', a.name,
  'baseUrl', COALESCE(a.credentials->>'base_url', ''),
  'apiKey', COALESCE(a.credentials->>'api_key', ''),
  'modelMapping', CASE WHEN jsonb_typeof(a.credentials->'model_mapping') = 'object'
    THEN a.credentials->'model_mapping' ELSE '{{}}'::jsonb END
) ORDER BY a.id), '[]'::jsonb)::text
FROM accounts a
WHERE a.id IN ({}) AND a.platform = 'openai' AND a.type IN ('apikey', 'api_key')
  AND a.deleted_at IS NULL;
COMMIT;
"#,
            id_list_sql(&ids)
        );
        let raw = self.run_psql(&sql)?;
        let payload = raw
            .lines()
            .rev()
            .map(str::trim)
            .find(|line| line.starts_with('['))
            .ok_or_else(|| "relay credential query returned no JSON".to_string())?;
        let rows: Vec<RawRelayTarget> = serde_json::from_str(payload)
            .map_err(|error| format!("parse ephemeral relay targets: {error}"))?;
        let mut targets = Vec::new();
        for row in rows {
            if row.base_url.trim().is_empty() || row.api_key.trim().is_empty() {
                continue;
            }
            let probe_model = resolve_model_mapping(&row.model_mapping, &model.client_model)
                .unwrap_or_else(|| model.expected_upstream_model.clone());
            targets.push(RelayProbeTarget {
                account_id: row.account_id,
                name: row.name,
                base_url: row.base_url,
                api_key: row.api_key,
                probe_model,
            });
        }
        Ok(targets)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RelayProbeOptions {
    /// GET /v1/models is read-only and enabled by default.
    pub probe_models: bool,
    /// POST /v1/responses consumes quota and must be explicitly enabled.
    pub probe_responses: bool,
}

impl Default for RelayProbeOptions {
    fn default() -> Self {
        Self {
            probe_models: true,
            probe_responses: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProbeCheck {
    pub attempted: bool,
    pub success: bool,
    pub status_code: Option<u16>,
    pub detail: String,
}

impl ProbeCheck {
    fn skipped(detail: &str) -> Self {
        Self {
            attempted: false,
            success: false,
            status_code: None,
            detail: detail.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RelayProbeResult {
    pub account_id: i64,
    pub account_name: String,
    pub upstream_host: String,
    pub models: ProbeCheck,
    pub responses: ProbeCheck,
}

/// Probe relays directly. Results contain status/count only, never response
/// bodies, Authorization headers, base URLs with userinfo, or API keys.
pub fn probe_relays(
    targets: Vec<RelayProbeTarget>,
    options: RelayProbeOptions,
) -> Vec<RelayProbeResult> {
    let client = match Client::builder()
        .timeout(PROBE_TIMEOUT)
        .user_agent("Codex-Provider-Hub/route-doctor")
        .build()
    {
        Ok(client) => client,
        Err(_) => {
            return targets
                .into_iter()
                .map(|target| RelayProbeResult {
                    account_id: target.account_id,
                    account_name: target.name,
                    upstream_host: "invalid".into(),
                    models: ProbeCheck {
                        attempted: options.probe_models,
                        success: false,
                        status_code: None,
                        detail: "could not initialize safe HTTP probe".into(),
                    },
                    responses: ProbeCheck {
                        attempted: options.probe_responses,
                        success: false,
                        status_code: None,
                        detail: "could not initialize safe HTTP probe".into(),
                    },
                })
                .collect();
        }
    };

    targets
        .into_iter()
        .map(|target| probe_one_relay(&client, target, options))
        .collect()
}

fn probe_one_relay(
    client: &Client,
    target: RelayProbeTarget,
    options: RelayProbeOptions,
) -> RelayProbeResult {
    let parsed = match reqwest::Url::parse(target.base_url.trim()) {
        Ok(url)
            if matches!(url.scheme(), "http" | "https")
                && url.username().is_empty()
                && url.password().is_none()
                && url.host_str().is_some() =>
        {
            url
        }
        _ => {
            return RelayProbeResult {
                account_id: target.account_id,
                account_name: target.name,
                upstream_host: "invalid".into(),
                models: ProbeCheck {
                    attempted: options.probe_models,
                    success: false,
                    status_code: None,
                    detail: "stored relay base URL is invalid".into(),
                },
                responses: ProbeCheck {
                    attempted: options.probe_responses,
                    success: false,
                    status_code: None,
                    detail: "stored relay base URL is invalid".into(),
                },
            };
        }
    };
    let upstream_host = match parsed.port() {
        Some(port) => format!("{}:{port}", parsed.host_str().unwrap_or("invalid")),
        None => parsed.host_str().unwrap_or("invalid").to_string(),
    };

    let models = if options.probe_models {
        match upstream_endpoint(&parsed, "models") {
            Ok(url) => match client.get(url).bearer_auth(&target.api_key).send() {
                Ok(response) => {
                    let status = response.status();
                    if !status.is_success() {
                        ProbeCheck {
                            attempted: true,
                            success: false,
                            status_code: Some(status.as_u16()),
                            detail: format!("GET /v1/models returned HTTP {}", status.as_u16()),
                        }
                    } else {
                        let count = response.json::<Value>().ok().and_then(|body| {
                            body.get("data").and_then(Value::as_array).map(Vec::len)
                        });
                        ProbeCheck {
                            attempted: true,
                            success: count.is_some(),
                            status_code: Some(status.as_u16()),
                            detail: count
                                .map(|count| format!("{count} model(s) returned"))
                                .unwrap_or_else(|| "HTTP 2xx but data[] is missing".into()),
                        }
                    }
                }
                Err(error) => ProbeCheck {
                    attempted: true,
                    success: false,
                    status_code: error.status().map(|status| status.as_u16()),
                    detail: if error.is_timeout() {
                        "GET /v1/models timed out".into()
                    } else {
                        "GET /v1/models request failed".into()
                    },
                },
            },
            Err(detail) => ProbeCheck {
                attempted: true,
                success: false,
                status_code: None,
                detail,
            },
        }
    } else {
        ProbeCheck::skipped("read-only model probe disabled")
    };

    let responses = if options.probe_responses {
        match upstream_endpoint(&parsed, "responses") {
            Ok(url) => {
                let payload = json!({
                    "model": target.probe_model,
                    "input": "Reply with OK.",
                    "max_output_tokens": 16,
                    "store": false,
                    "stream": true,
                });
                match client
                    .post(url)
                    .bearer_auth(&target.api_key)
                    .json(&payload)
                    .send()
                {
                    Ok(response) => {
                        let status = response.status();
                        ProbeCheck {
                            attempted: true,
                            success: status.is_success(),
                            status_code: Some(status.as_u16()),
                            detail: if status.is_success() {
                                "minimal streamed response accepted".into()
                            } else {
                                format!("POST /v1/responses returned HTTP {}", status.as_u16())
                            },
                        }
                    }
                    Err(error) => ProbeCheck {
                        attempted: true,
                        success: false,
                        status_code: error.status().map(|status| status.as_u16()),
                        detail: if error.is_timeout() {
                            "POST /v1/responses timed out".into()
                        } else {
                            "POST /v1/responses request failed".into()
                        },
                    },
                }
            }
            Err(detail) => ProbeCheck {
                attempted: true,
                success: false,
                status_code: None,
                detail,
            },
        }
    } else {
        ProbeCheck::skipped("quota-consuming responses probe not requested")
    };

    RelayProbeResult {
        account_id: target.account_id,
        account_name: target.name,
        upstream_host,
        models,
        responses,
    }
}

fn upstream_endpoint(base: &reqwest::Url, leaf: &str) -> Result<reqwest::Url, String> {
    let mut url = base.clone();
    url.set_query(None);
    url.set_fragment(None);
    let path = url.path().trim_end_matches('/');
    let next = if path.ends_with("/v1") {
        format!("{path}/{leaf}")
    } else if path.is_empty() {
        format!("/v1/{leaf}")
    } else {
        format!("{path}/v1/{leaf}")
    };
    url.set_path(&next);
    Ok(url)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RouteDoctorCommandResult {
    pub report: DiagnosisReport,
    pub relay_probes: Vec<RelayProbeResult>,
    pub captured_at: DateTime<Utc>,
}

fn runtime_route_doctor() -> Result<(DockerRouteDoctor, i64, ModelContext), String> {
    let sub2api_dir = crate::gateway::sub2api_dir();
    let doctor = DockerRouteDoctor::new(&sub2api_dir);
    let gateway_key = crate::gateway::read_gateway_key()?;
    let api_key_id = doctor.resolve_api_key_id(&gateway_key)?;
    let model = model_context_from_codex_config(&crate::gateway::codex_config_path())?;
    Ok((doctor, api_key_id, model))
}

fn relay_account_ids(snapshot: &DoctorSnapshot) -> Vec<i64> {
    snapshot
        .accounts
        .iter()
        .filter(|account| account.is_relay())
        .map(|account| account.id)
        .collect()
}

/// Full read-only diagnosis. The only network side effect is GET /v1/models
/// against configured relays; no quota-consuming response is generated.
#[tauri::command]
pub fn diagnose_sub2api_route() -> Result<RouteDoctorCommandResult, String> {
    let (doctor, api_key_id, model) = runtime_route_doctor()?;
    let snapshot = doctor.load_snapshot(api_key_id, model.clone())?;
    let report = diagnose(&snapshot, Utc::now());
    let ids = relay_account_ids(&snapshot);
    let targets = if ids.is_empty() {
        Vec::new()
    } else {
        doctor.load_relay_probe_targets(&ids, &model)?
    };
    let relay_probes = probe_relays(targets, RelayProbeOptions::default());
    Ok(RouteDoctorCommandResult {
        report,
        relay_probes,
        captured_at: Utc::now(),
    })
}

/// Explicit relay probe command. `probe_responses=true` must originate from a
/// separately confirmed UI action because it consumes a small amount of quota.
#[tauri::command]
pub fn probe_sub2api_route_relays(probe_responses: bool) -> Result<Vec<RelayProbeResult>, String> {
    let (doctor, api_key_id, model) = runtime_route_doctor()?;
    let snapshot = doctor.load_snapshot(api_key_id, model.clone())?;
    let ids = relay_account_ids(&snapshot);
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let targets = doctor.load_relay_probe_targets(&ids, &model)?;
    Ok(probe_relays(
        targets,
        RelayProbeOptions {
            probe_models: true,
            probe_responses,
        },
    ))
}

/// Dry-run by default; apply requires the exact backend confirmation phrase.
#[tauri::command]
pub fn repair_sub2api_route(
    action: RepairAction,
    apply: bool,
    confirmation: Option<String>,
) -> Result<RepairResult, String> {
    let (doctor, api_key_id, model) = runtime_route_doctor()?;
    doctor.repair(
        api_key_id,
        model,
        RepairRequest {
            action,
            mode: if apply {
                RepairMode::Apply
            } else {
                RepairMode::DryRun
            },
            actor: "local-hub-user".into(),
            confirmation,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 8, 11, 12, 0, 0)
            .single()
            .unwrap()
    }

    fn group(id: i64, name: &str) -> GroupSnapshot {
        GroupSnapshot {
            id,
            name: name.into(),
            status: "active".into(),
            platform: "openai".into(),
            require_oauth_only: false,
            fallback_group_id: None,
            supported_model_scopes: vec![],
            supported_model_scopes_json_null: false,
            supported_model_scopes_sql_null: false,
            supported_model_scopes_invalid: false,
        }
    }

    fn account(id: i64, name: &str, account_type: &str) -> AccountSnapshot {
        AccountSnapshot {
            id,
            name: name.into(),
            account_type: account_type.into(),
            status: "active".into(),
            schedulable: true,
            rate_limit_reset_at: None,
            overload_until: None,
            temp_unschedulable_until: None,
            model_mapping: json!({"sub2api-gpt-5.6-sol": "gpt-5.6-sol"}),
            model_rate_limits: json!({}),
        }
    }

    fn snapshot() -> DoctorSnapshot {
        DoctorSnapshot {
            api_key: Some(ApiKeySnapshot {
                id: 1,
                name: "json-direct-proxy".into(),
                status: "active".into(),
                group_id: Some(2),
            }),
            groups: vec![group(2, "production"), group(3, "known-good")],
            accounts: vec![
                account(4, "OAuth A", "oauth"),
                account(5, "AIHub", "apikey"),
                account(6, "AnyRouter", "apikey"),
            ],
            memberships: vec![
                MembershipSnapshot {
                    account_id: 4,
                    group_id: 2,
                    priority: 10,
                },
                MembershipSnapshot {
                    account_id: 5,
                    group_id: 2,
                    priority: 100,
                },
                MembershipSnapshot {
                    account_id: 6,
                    group_id: 3,
                    priority: 10,
                },
            ],
            model: ModelContext::new("sub2api-gpt-5.6-sol", "gpt-5.6-sol").unwrap(),
            captured_at: now(),
        }
    }

    fn finding<'a>(report: &'a DiagnosisReport, code: &str) -> &'a DiagnosisIssue {
        report
            .issues
            .iter()
            .find(|finding| finding.code == code)
            .unwrap_or_else(|| panic!("missing finding {code}: {:?}", report.issues))
    }

    #[test]
    fn healthy_group_has_oauth_and_relay_fallback() {
        let report = diagnose(&snapshot(), now());
        assert!(report.healthy, "{:?}", report.issues);
        assert_eq!(report.usable_member_count, 2);
        assert!(report.issues.is_empty());
    }

    #[test]
    fn detects_json_literal_null_not_only_sql_null() {
        let mut state = snapshot();
        state.groups[0].supported_model_scopes_json_null = true;
        let report = diagnose(&state, now());
        let issue = finding(&report, "supported_model_scopes_json_null");
        assert_eq!(issue.severity, IssueSeverity::Critical);
        assert_eq!(
            issue.repair,
            Some(RepairAction::SetGroupSupportedScopes {
                group_id: 2,
                scopes: vec![]
            })
        );
    }

    #[test]
    fn oauth_only_filter_is_flagged_even_when_relay_is_a_member() {
        let mut state = snapshot();
        state.groups[0].require_oauth_only = true;
        let report = diagnose(&state, now());
        let issue = finding(&report, "relay_fallback_filtered");
        assert_eq!(issue.account_ids, vec![5]);
        assert_eq!(
            issue.repair,
            Some(RepairAction::SetGroupRequireOauthOnly {
                group_id: 2,
                enabled: false
            })
        );
    }

    #[test]
    fn disabled_and_model_parked_members_explain_zero_pool() {
        let mut state = snapshot();
        state.accounts[0].schedulable = false;
        state.accounts[1].model_rate_limits = json!({
            "gpt-5.6-sol": {"rate_limit_reset_at": "2026-08-11T13:00:00Z"}
        });
        let report = diagnose(&state, now());
        assert_eq!(
            finding(&report, "members_not_schedulable").account_ids,
            vec![4]
        );
        assert_eq!(finding(&report, "members_parked").account_ids, vec![5]);
        assert_eq!(report.usable_member_count, 0);
        finding(&report, "group_has_no_usable_accounts");
    }

    #[test]
    fn missing_mapping_is_critical_for_prefixed_model() {
        let mut state = snapshot();
        state.accounts[0].model_mapping = json!({});
        state.accounts[1].model_mapping = json!({});
        let issue = finding(&diagnose(&state, now()), "current_model_mapping_missing").clone();
        assert_eq!(issue.severity, IssueSeverity::Critical);
        assert_eq!(issue.account_ids, vec![4, 5]);
        assert!(matches!(
            issue.repair,
            Some(RepairAction::AddModelMapping { .. })
        ));
    }

    #[test]
    fn missing_current_group_plans_move_to_verified_usable_group() {
        let mut state = snapshot();
        state.api_key.as_mut().unwrap().group_id = Some(99);
        let report = diagnose(&state, now());
        assert_eq!(
            finding(&report, "api_key_group_not_found").repair,
            Some(RepairAction::MoveApiKeyToGroup {
                api_key_id: 1,
                target_group_id: 2
            })
        );
    }

    #[test]
    fn repair_plan_records_original_value_for_scopes() {
        let mut state = snapshot();
        state.groups[0].supported_model_scopes_json_null = true;
        let plan = build_repair_plan(
            &state,
            &RepairAction::SetGroupSupportedScopes {
                group_id: 2,
                scopes: vec![],
            },
            now(),
        )
        .unwrap();
        assert_eq!(plan.changes[0].old_value, Value::Null);
        assert_eq!(plan.changes[0].new_value, json!([]));
        assert!(plan.backup_required && plan.app_restart_required);
    }

    #[test]
    fn move_plan_rejects_an_unusable_target_group() {
        let mut state = snapshot();
        state.accounts[2].schedulable = false;
        let error = build_repair_plan(
            &state,
            &RepairAction::MoveApiKeyToGroup {
                api_key_id: 1,
                target_group_id: 3,
            },
            now(),
        )
        .unwrap_err();
        assert!(error.contains("no usable account"));
    }

    #[test]
    fn snapshot_serialization_omits_mapping_and_parking_raw_data() {
        let mut state = snapshot();
        state.accounts[0].model_mapping = json!({"secret-marker": "should-not-render"});
        state.accounts[0].model_rate_limits = json!({"secret-marker": {}});
        let encoded = serde_json::to_string(&state).unwrap();
        assert!(!encoded.contains("secret-marker"));
        assert!(!encoded.contains("modelMapping"));
        assert!(!encoded.contains("modelRateLimits"));
    }

    #[test]
    fn relay_target_debug_redacts_api_key_and_responses_default_off() {
        let target = RelayProbeTarget {
            account_id: 5,
            name: "AIHub".into(),
            base_url: "https://example.test/v1".into(),
            api_key: "secret-marker".into(),
            probe_model: "gpt-5.6-sol".into(),
        };
        let debug = format!("{target:?}");
        assert!(!debug.contains("secret-marker"));
        assert!(debug.contains("[REDACTED]"));
        let options = RelayProbeOptions::default();
        assert!(options.probe_models);
        assert!(!options.probe_responses);
    }

    #[test]
    fn upstream_endpoint_preserves_custom_prefix_and_avoids_duplicate_v1() {
        let base = reqwest::Url::parse("https://relay.test/openai/v1/").unwrap();
        assert_eq!(
            upstream_endpoint(&base, "models").unwrap().as_str(),
            "https://relay.test/openai/v1/models"
        );
        let root = reqwest::Url::parse("https://relay.test").unwrap();
        assert_eq!(
            upstream_endpoint(&root, "responses").unwrap().as_str(),
            "https://relay.test/v1/responses"
        );
    }

    #[test]
    fn wildcard_mapping_is_recognized() {
        let mapping = json!({"sub2api-gpt-*": "gpt-5.6-sol"});
        assert_eq!(
            resolve_model_mapping(&mapping, "sub2api-gpt-5.6-sol").as_deref(),
            Some("gpt-5.6-sol")
        );
    }

    #[test]
    fn runtime_error_redacts_credentials() {
        assert_eq!(
            safe_runtime_error("Authorization: Bearer secret-marker"),
            "operation failed (sensitive upstream detail withheld)"
        );
    }

    #[test]
    fn repair_action_contract_is_camel_case_for_tauri() {
        let encoded = serde_json::to_value(RepairAction::AddModelMapping {
            account_ids: vec![4, 5],
            client_model: "sub2api-gpt-5.6-sol".into(),
            upstream_model: "gpt-5.6-sol".into(),
        })
        .unwrap();
        assert_eq!(encoded["action"], "addModelMapping");
        assert_eq!(encoded["accountIds"], json!([4, 5]));
        assert_eq!(encoded["clientModel"], "sub2api-gpt-5.6-sol");
        assert!(encoded.get("account_ids").is_none());
    }

    #[test]
    fn docker_test_constructor_does_not_touch_runtime() {
        let doctor = DockerRouteDoctor::with_paths(
            PathBuf::from("/not/run/docker"),
            "postgres-test",
            "app-test",
            PathBuf::from("/tmp/not-created"),
            PathBuf::from("/tmp/not-created/audit.jsonl"),
        );
        assert_eq!(doctor.postgres_container, "postgres-test");
        assert_eq!(doctor.app_container, "app-test");
    }

    #[test]
    #[ignore = "requires the live local Sub2API Docker deployment; read-only"]
    fn live_readonly_snapshot_contract() {
        let doctor = DockerRouteDoctor::new(Path::new("/tmp/route-doctor-readonly-test"));
        let state = doctor
            .load_snapshot(
                1,
                ModelContext::new("sub2api-gpt-5.6-sol", "gpt-5.6-sol").unwrap(),
            )
            .unwrap();
        assert_eq!(state.api_key.as_ref().map(|key| key.id), Some(1));
        assert!(state
            .api_key
            .as_ref()
            .and_then(|key| key.group_id)
            .is_some());
        assert!(!state.groups.is_empty());
        assert!(!state.memberships.is_empty());
    }
}
