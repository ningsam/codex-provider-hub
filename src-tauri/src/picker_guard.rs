//! Codex model-picker guard: keep Statsig `use_hidden_models` false in ChatGPT Local Storage.
//!
//! ChatGPT.app filters custom slugs (`aihub-*` / `anyrouter-*` / `sub2api-*`) when Statsig
//! dynamic config `107580212` has `use_hidden_models=true`. The value lives in Chromium
//! Local Storage (LevelDB), often inside Snappy-compressed `.ldb` blocks (plain byte scan
//! can miss it). Primary writer: bundled `picker_guard_patch.py` + plyvel. Fallback: in-file
//! UTF-8 / UTF-16 byte replace. Reopen always uses Statsig `--host-rules` so CDN cannot
//! flip the flag back.

use crate::http_util::now_iso;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tauri::{AppHandle, Manager};

/// Statsig can rewrite the flag within ~30s; poll faster than that.
const POLL_SECS: u64 = 45;
const CONFIG_ID: &str = "107580212";
/// Electron host-rules that keep Statsig from overwriting the local patch.
const STATSIG_HOST_RULES: &str = "MAP api.statsigcdn.com 127.0.0.1, MAP featureassets.org 127.0.0.1, MAP prodregistryv2.org 127.0.0.1, MAP statsigapi.net 127.0.0.1, MAP api.statsig.com 127.0.0.1";
const CHATGPT_BIN: &str = "/Applications/ChatGPT.app/Contents/MacOS/ChatGPT";
const CHATGPT_PGREP: &str = "ChatGPT.app/Contents/MacOS/ChatGPT";

static FORCE_QUIT_ENABLED: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PickerGuardStatus {
    pub enabled: bool,
    /// `Some(true)` / `Some(false)` when found; `None` if not present in cache.
    pub use_hidden_models: Option<bool>,
    pub patched_at: Option<String>,
    pub chatgpt_running: bool,
    /// True when the main ChatGPT process cmdline includes `--host-rules` / Statsig MAP.
    pub host_rules_active: bool,
    pub leveldb_path: String,
    pub last_error: Option<String>,
    /// Soft hint for UI when ChatGPT holds the LevelDB lock or runs unguarded.
    pub pending_fix: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PickerGuardConfig {
    /// Default true — auto-patch when ChatGPT is not running.
    #[serde(default = "default_true")]
    enabled: bool,
    /// Only when true: quit ChatGPT → patch → reopen during background/apply.
    #[serde(default)]
    force_quit: bool,
    #[serde(default)]
    patched_at: Option<String>,
    #[serde(default)]
    last_error: Option<String>,
}

fn default_true() -> bool {
    true
}

impl Default for PickerGuardConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            force_quit: false,
            patched_at: None,
            last_error: None,
        }
    }
}

struct RuntimeState {
    pending_fix: bool,
}

static RUNTIME: Mutex<RuntimeState> = Mutex::new(RuntimeState { pending_fix: false });

fn home_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"))
}

pub fn leveldb_dir() -> PathBuf {
    home_dir().join("Library/Application Support/Codex/Default/Local Storage/leveldb")
}

fn local_storage_dir() -> PathBuf {
    home_dir().join("Library/Application Support/Codex/Default/Local Storage")
}

fn config_path_from_app(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("resolve app data dir: {e}"))?;
    fs::create_dir_all(&dir).map_err(|e| format!("create app data dir: {e}"))?;
    Ok(dir.join("picker_guard.json"))
}

fn config_path_fallback() -> PathBuf {
    home_dir()
        .join("Library/Application Support/com.skylerenzi.codex-provider-hub")
        .join("picker_guard.json")
}

fn resolve_config_path(app: Option<&AppHandle>) -> PathBuf {
    if let Some(app) = app {
        if let Ok(p) = config_path_from_app(app) {
            return p;
        }
    }
    let p = config_path_fallback();
    if let Some(parent) = p.parent() {
        let _ = fs::create_dir_all(parent);
    }
    p
}

fn load_config(app: Option<&AppHandle>) -> PickerGuardConfig {
    let path = resolve_config_path(app);
    if !path.exists() {
        let cfg = PickerGuardConfig::default();
        let _ = save_config(app, &cfg);
        return cfg;
    }
    match fs::read_to_string(&path) {
        Ok(raw) => serde_json::from_str(&raw).unwrap_or_default(),
        Err(_) => PickerGuardConfig::default(),
    }
}

fn save_config(app: Option<&AppHandle>, cfg: &PickerGuardConfig) -> Result<(), String> {
    let path = resolve_config_path(app);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create config dir: {e}"))?;
    }
    let raw = serde_json::to_string_pretty(cfg).map_err(|e| format!("serialize config: {e}"))?;
    fs::write(&path, raw).map_err(|e| format!("write {}: {e}", path.display()))
}

fn chatgpt_running() -> bool {
    Command::new("pgrep")
        .args(["-f", CHATGPT_PGREP])
        .output()
        .map(|o| o.status.success() && !o.stdout.is_empty())
        .unwrap_or(false)
}

/// Full cmdline of the main ChatGPT process (empty if not running).
fn chatgpt_main_cmdline() -> Option<String> {
    let output = Command::new("pgrep")
        .args(["-lf", CHATGPT_PGREP])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    for line in text.lines() {
        // Prefer the bare MacOS/ChatGPT binary line (not helpers).
        if line.contains("/Contents/MacOS/ChatGPT") {
            return Some(line.to_string());
        }
    }
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.lines().next().unwrap_or("").to_string())
    }
}

fn chatgpt_host_rules_active() -> bool {
    let Some(cmd) = chatgpt_main_cmdline() else {
        return false;
    };
    cmd.contains("--host-rules")
        || cmd.contains("host-rules=")
        || cmd.contains("api.statsigcdn.com 127.0.0.1")
}

fn quit_chatgpt() -> Result<(), String> {
    let _ = Command::new("osascript")
        .args(["-e", "quit app \"ChatGPT\""])
        .output();
    for _ in 0..20 {
        std::thread::sleep(Duration::from_millis(250));
        if !chatgpt_running() {
            let _ = Command::new("pkill")
                .args(["-f", "ChatGPT.app/Contents/MacOS"])
                .output();
            std::thread::sleep(Duration::from_millis(400));
            return Ok(());
        }
    }
    let _ = Command::new("pkill")
        .args(["-9", "-f", "ChatGPT.app/Contents/MacOS"])
        .output();
    std::thread::sleep(Duration::from_millis(500));
    if chatgpt_running() {
        return Err("ChatGPT 仍在运行，无法安全写入 Local Storage".into());
    }
    Ok(())
}

/// Always launch ChatGPT with Statsig host-rules (permanent anti-refresh).
fn open_chatgpt() {
    let bin = PathBuf::from(CHATGPT_BIN);
    if bin.is_file() {
        let _ = Command::new(&bin)
            .arg(format!("--host-rules={STATSIG_HOST_RULES}"))
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        return;
    }
    let _ = Command::new("open")
        .args([
            "-na",
            "ChatGPT",
            "--args",
            &format!("--host-rules={STATSIG_HOST_RULES}"),
        ])
        .output();
}

fn utf16_le(s: &str) -> Vec<u8> {
    s.encode_utf16().flat_map(|u| u.to_le_bytes()).collect()
}

/// Replacement pairs (from → to) as Unicode strings; applied as UTF-8 and UTF-16-LE.
fn replacement_pairs() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            r#"use_hidden_models\":true"#,
            r#"use_hidden_models\":false"#,
        ),
        (
            r#"use_hidden_models\": true"#,
            r#"use_hidden_models\":false"#,
        ),
        (
            r#"use_hidden_models\":\"true"#,
            r#"use_hidden_models\":\"false"#,
        ),
        (
            r#"use_hidden_models\": \"true"#,
            r#"use_hidden_models\":\"false"#,
        ),
        (r#"use_hidden_models":true"#, r#"use_hidden_models":false"#),
        (r#"use_hidden_models": true"#, r#"use_hidden_models":false"#),
        (
            r#"use_hidden_models":"true"#,
            r#"use_hidden_models":"false"#,
        ),
        (
            r#"use_hidden_models": "true"#,
            r#"use_hidden_models":"false"#,
        ),
    ]
}

fn scan_use_hidden_models(dir: &Path) -> Option<bool> {
    if !dir.is_dir() {
        return None;
    }
    let true_needles: Vec<Vec<u8>> = [
        r#"use_hidden_models\":true"#,
        r#"use_hidden_models\":\"true"#,
        r#"use_hidden_models":true"#,
        r#"use_hidden_models":"true"#,
    ]
    .into_iter()
    .flat_map(|s| vec![s.as_bytes().to_vec(), utf16_le(s)])
    .collect();

    let false_needles: Vec<Vec<u8>> = [
        r#"use_hidden_models\":false"#,
        r#"use_hidden_models\":\"false"#,
        r#"use_hidden_models":false"#,
        r#"use_hidden_models":"false"#,
    ]
    .into_iter()
    .flat_map(|s| vec![s.as_bytes().to_vec(), utf16_le(s)])
    .collect();

    let mut saw_true = false;
    let mut saw_false = false;

    let Ok(entries) = fs::read_dir(dir) else {
        return None;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if !(name.ends_with(".ldb") || name.ends_with(".log")) {
            continue;
        }
        let Ok(data) = fs::read(&path) else {
            continue;
        };
        if true_needles.iter().any(|n| find_bytes(&data, n).is_some()) {
            saw_true = true;
        }
        if false_needles.iter().any(|n| find_bytes(&data, n).is_some()) {
            saw_false = true;
        }
    }

    if saw_true {
        Some(true)
    } else if saw_false {
        Some(false)
    } else {
        let _ = CONFIG_ID;
        None
    }
}

fn find_bytes(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

fn replace_all(data: &mut Vec<u8>, from: &[u8], to: &[u8]) -> usize {
    if from.is_empty() || from == to {
        return 0;
    }
    let mut count = 0;
    let mut out = Vec::with_capacity(data.len());
    let mut i = 0;
    while i < data.len() {
        if i + from.len() <= data.len() && &data[i..i + from.len()] == from {
            out.extend_from_slice(to);
            i += from.len();
            count += 1;
        } else {
            out.push(data[i]);
            i += 1;
        }
    }
    if count > 0 {
        *data = out;
    }
    count
}

fn backup_local_storage() -> Result<PathBuf, String> {
    let src = local_storage_dir();
    if !src.is_dir() {
        return Err(format!("Local Storage missing: {}", src.display()));
    }
    let ts = chrono::Local::now().format("%Y%m%d_%H%M%S");
    let dest = home_dir().join(format!(
        "Library/Application Support/Codex/Default/Local Storage.bak.{ts}"
    ));
    copy_dir_recursive(&src, &dest)?;
    Ok(dest)
}

fn copy_dir_recursive(src: &Path, dest: &Path) -> Result<(), String> {
    fs::create_dir_all(dest).map_err(|e| format!("mkdir {}: {e}", dest.display()))?;
    for entry in fs::read_dir(src).map_err(|e| format!("read_dir {}: {e}", src.display()))? {
        let entry = entry.map_err(|e| format!("dir entry: {e}"))?;
        let from = entry.path();
        let to = dest.join(entry.file_name());
        if from.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else {
            fs::copy(&from, &to)
                .map_err(|e| format!("copy {} → {}: {e}", from.display(), to.display()))?;
        }
    }
    Ok(())
}

fn patch_leveldb_files(dir: &Path) -> Result<usize, String> {
    if !dir.is_dir() {
        return Err(format!("leveldb missing: {}", dir.display()));
    }
    let pairs = replacement_pairs();
    let mut encoded: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
    for (frm, to) in &pairs {
        encoded.push((frm.as_bytes().to_vec(), to.as_bytes().to_vec()));
        encoded.push((utf16_le(frm), utf16_le(to)));
    }

    let mut total = 0usize;
    for entry in fs::read_dir(dir).map_err(|e| format!("read leveldb: {e}"))? {
        let entry = entry.map_err(|e| format!("leveldb entry: {e}"))?;
        let path = entry.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if !(name.ends_with(".ldb") || name.ends_with(".log")) {
            continue;
        }
        let mut file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|e| format!("open {}: {e}", path.display()))?;
        let mut data = Vec::new();
        file.read_to_end(&mut data)
            .map_err(|e| format!("read {}: {e}", path.display()))?;
        let mut file_hits = 0usize;
        for (from, to) in &encoded {
            file_hits += replace_all(&mut data, from, to);
        }
        if file_hits == 0 {
            continue;
        }
        file.set_len(0)
            .map_err(|e| format!("truncate {}: {e}", path.display()))?;
        file.write_all(&data)
            .map_err(|e| format!("write {}: {e}", path.display()))?;
        file.flush()
            .map_err(|e| format!("flush {}: {e}", path.display()))?;
        total += file_hits;
    }
    Ok(total)
}

fn resolve_patch_script(app: Option<&AppHandle>) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(app) = app {
        if let Ok(dir) = app.path().resource_dir() {
            candidates.push(dir.join("resources/picker_guard_patch.py"));
            candidates.push(dir.join("picker_guard_patch.py"));
        }
    }
    // Dev / source tree
    candidates
        .push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/picker_guard_patch.py"));
    candidates.push(
        home_dir().join("Documents/codex-provider-hub/src-tauri/resources/picker_guard_patch.py"),
    );
    candidates.into_iter().find(|p| p.is_file())
}

fn resolve_plyvel_python() -> Option<PathBuf> {
    let mut candidates = vec![
        home_dir().join("Documents/codex-provider-hub/.venv-picker/bin/python"),
        home_dir().join("Documents/codex-provider-hub/.venv-picker/bin/python3"),
        home_dir().join(
            "Library/Application Support/com.skylerenzi.codex-provider-hub/picker-venv/bin/python",
        ),
    ];
    for name in ["python3.12", "python3", "python"] {
        if let Ok(output) = Command::new("which").arg(name).output() {
            if output.status.success() {
                let p = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !p.is_empty() {
                    candidates.push(PathBuf::from(p));
                }
            }
        }
    }
    for py in candidates {
        if !py.is_file() {
            continue;
        }
        let ok = Command::new(&py)
            .args(["-c", "import plyvel"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if ok {
            return Some(py);
        }
    }
    None
}

/// Primary writer: python + plyvel (handles Snappy-compressed .ldb values).
fn patch_via_python(app: Option<&AppHandle>, db: &Path) -> Result<String, String> {
    let script = resolve_patch_script(app)
        .ok_or_else(|| "找不到 picker_guard_patch.py（请确认已 bundle resources）".to_string())?;
    let py = resolve_plyvel_python().ok_or_else(|| {
        "找不到带 plyvel 的 Python（期望 Documents/codex-provider-hub/.venv-picker）".to_string()
    })?;
    let output = Command::new(&py)
        .arg(&script)
        .arg("--db")
        .arg(db)
        .output()
        .map_err(|e| format!("spawn python patch: {e}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if !output.status.success() {
        return Err(format!(
            "python patch failed (exit {:?}): {} {}",
            output.status.code(),
            stdout,
            stderr
        ));
    }
    // Prefer JSON ok field when present.
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&stdout) {
        if v.get("ok").and_then(|x| x.as_bool()) == Some(false) {
            return Err(format!("python patch reported failure: {stdout}"));
        }
        let still = v.get("still_true").and_then(|x| x.as_i64()).unwrap_or(0);
        if still > 0 {
            return Err(format!("python patch still_true={still}: {stdout}"));
        }
    }
    Ok(stdout)
}

/// Combined patch: python/plyvel first, byte-replace fallback.
fn patch_leveldb(app: Option<&AppHandle>, dir: &Path) -> Result<(usize, String), String> {
    match patch_via_python(app, dir) {
        Ok(msg) => {
            let n = serde_json::from_str::<serde_json::Value>(&msg)
                .ok()
                .and_then(|v| v.get("patched").and_then(|x| x.as_u64()))
                .unwrap_or(1) as usize;
            return Ok((n, format!("plyvel: {msg}")));
        }
        Err(py_err) => {
            let n = patch_leveldb_files(dir)?;
            if n == 0 {
                return Err(format!(
                    "plyvel 失败且字节替换未命中（值可能在压缩块内）: {py_err}"
                ));
            }
            Ok((n, format!("byte-fallback hits={n}; plyvel_err={py_err}")))
        }
    }
}

fn build_status(_app: Option<&AppHandle>, cfg: &PickerGuardConfig) -> PickerGuardStatus {
    let leveldb = leveldb_dir();
    let running = chatgpt_running();
    let host_rules = running && chatgpt_host_rules_active();
    let use_hidden = scan_use_hidden_models(&leveldb);
    let unguarded = running && !host_rules;
    let pending = RUNTIME.lock().pending_fix
        || unguarded
        || (cfg.enabled && use_hidden == Some(true) && running && !cfg.force_quit);

    PickerGuardStatus {
        enabled: cfg.enabled,
        use_hidden_models: use_hidden,
        patched_at: cfg.patched_at.clone(),
        chatgpt_running: running,
        host_rules_active: host_rules,
        leveldb_path: leveldb.display().to_string(),
        last_error: cfg.last_error.clone(),
        pending_fix: pending,
    }
}

/// Full guarded cycle: quit → backup → plyvel/byte patch → reopen with host-rules.
fn relaunch_guarded_internal(app: Option<&AppHandle>) -> Result<PickerGuardStatus, String> {
    let mut cfg = load_config(app);
    let leveldb = leveldb_dir();
    let was_running = chatgpt_running();

    if was_running {
        if let Err(e) = quit_chatgpt() {
            cfg.last_error = Some(e.clone());
            let _ = save_config(app, &cfg);
            return Err(e);
        }
    }

    let backup = backup_local_storage();
    if let Err(e) = &backup {
        cfg.last_error = Some(format!("备份失败: {e}"));
        let _ = save_config(app, &cfg);
        return Err(format!("备份 Local Storage 失败: {e}"));
    }

    match patch_leveldb(app, &leveldb) {
        Ok((_n, detail)) => {
            let value = scan_use_hidden_models(&leveldb);
            // Prefer plyvel JSON truth; byte-scan may miss compressed false.
            if value == Some(true) {
                let msg = format!("补丁后扫描仍为 true · {detail}");
                cfg.last_error = Some(msg.clone());
                let _ = save_config(app, &cfg);
                open_chatgpt();
                return Err(msg);
            }
            cfg.patched_at = Some(now_iso());
            cfg.last_error = None;
            RUNTIME.lock().pending_fix = false;
            let _ = save_config(app, &cfg);
            open_chatgpt();
            // Brief wait so status reflects host-rules process.
            std::thread::sleep(Duration::from_millis(800));
            Ok(build_status(app, &cfg))
        }
        Err(e) => {
            cfg.last_error = Some(e.clone());
            let _ = save_config(app, &cfg);
            if was_running {
                open_chatgpt();
            }
            Err(e)
        }
    }
}

/// Apply patch.
/// 1) Always try a live LevelDB write first (works when WAL is writable / python can lock).
/// 2) If still `true` or ChatGPT runs without host-rules and quit allowed → full relaunch.
fn apply_internal(
    app: Option<&AppHandle>,
    allow_quit: bool,
    reopen: bool,
) -> Result<PickerGuardStatus, String> {
    let mut cfg = load_config(app);
    FORCE_QUIT_ENABLED.store(cfg.force_quit, Ordering::Relaxed);

    let leveldb = leveldb_dir();
    let was_running = chatgpt_running();
    let unguarded = was_running && !chatgpt_host_rules_active();

    // If user/background wants a full guarded relaunch, or process is unguarded: do it.
    if reopen && allow_quit && (unguarded || scan_use_hidden_models(&leveldb) == Some(true)) {
        return relaunch_guarded_internal(app);
    }

    // Cheap path: patch in place without quitting.
    let live = patch_leveldb(app, &leveldb);
    let after_live = scan_use_hidden_models(&leveldb);
    if let Ok((_n, _detail)) = &live {
        if after_live == Some(false) || after_live != Some(true) {
            // If ChatGPT is unguarded we still need relaunch for permanent protection.
            if unguarded && allow_quit && reopen {
                return relaunch_guarded_internal(app);
            }
            cfg.patched_at = Some(now_iso());
            cfg.last_error = if unguarded {
                Some("缓存已修复，但当前 ChatGPT 未带 host-rules，点「立即修复」防刷新启动".into())
            } else {
                None
            };
            RUNTIME.lock().pending_fix = unguarded;
            let _ = save_config(app, &cfg);
            return Ok(build_status(app, &cfg));
        }
    }

    if was_running && !allow_quit && !cfg.force_quit {
        RUNTIME.lock().pending_fix = true;
        cfg.last_error = Some(if unguarded {
            "当前 ChatGPT 未防刷新，点立即修复".into()
        } else {
            "热补丁未生效且 ChatGPT 仍在运行；将在退出后自动修复，或点「立即修复」".into()
        });
        let _ = save_config(app, &cfg);
        return Ok(build_status(app, &cfg));
    }

    if allow_quit || !was_running {
        return relaunch_guarded_internal(app);
    }

    Err(live.err().unwrap_or_else(|| "无法应用选择器守护".into()))
}

fn background_tick() {
    let cfg = load_config(None);
    FORCE_QUIT_ENABLED.store(cfg.force_quit, Ordering::Relaxed);
    if !cfg.enabled {
        return;
    }
    let leveldb = leveldb_dir();
    let value = scan_use_hidden_models(&leveldb);
    let running = chatgpt_running();
    let unguarded = running && !chatgpt_host_rules_active();

    if unguarded {
        RUNTIME.lock().pending_fix = true;
        let mut cfg = cfg;
        cfg.last_error = Some("当前 ChatGPT 未防刷新，点立即修复".into());
        let _ = save_config(None, &cfg);
        // Do not force-quit in background unless force_quit is on.
        if cfg.force_quit {
            let _ = relaunch_guarded_internal(None);
        }
        return;
    }

    match value {
        Some(true) => {
            let allow_quit = cfg.force_quit || !running;
            let _ = apply_internal(None, allow_quit, cfg.force_quit);
        }
        Some(false) => {
            if !unguarded {
                RUNTIME.lock().pending_fix = false;
            }
            if cfg
                .last_error
                .as_deref()
                .is_some_and(|m| m.contains("退出后") || m.contains("未防刷新"))
                && !unguarded
            {
                let mut cfg = cfg;
                cfg.last_error = None;
                let _ = save_config(None, &cfg);
            }
        }
        None => {
            if RUNTIME.lock().pending_fix && !running {
                let _ = apply_internal(None, false, false);
            }
        }
    }
}

/// Spawn: on start, if the flag is already `true` or ChatGPT runs unguarded with force,
/// do a full quit→patch→reopen. Then poll every 45s.
pub fn spawn_background_loop(_app: &AppHandle) {
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_secs(2));
        let cfg = load_config(None);
        if cfg.enabled {
            let running = chatgpt_running();
            let host_rules = running && chatgpt_host_rules_active();
            let unguarded = running && !host_rules;
            // Byte-scan over .ldb can false-positive on Snappy leftovers; do not
            // hard-relaunch a process that already has Statsig host-rules.
            if host_rules {
                RUNTIME.lock().pending_fix = false;
            } else if unguarded && cfg.force_quit {
                let _ = relaunch_guarded_internal(None);
            } else if !running && scan_use_hidden_models(&leveldb_dir()) == Some(true) {
                let _ = relaunch_guarded_internal(None);
            } else {
                background_tick();
            }
        }
        loop {
            std::thread::sleep(Duration::from_secs(POLL_SECS));
            background_tick();
        }
    });
}

#[tauri::command]
pub fn get_picker_guard_status(app: AppHandle) -> Result<PickerGuardStatus, String> {
    let cfg = load_config(Some(&app));
    Ok(build_status(Some(&app), &cfg))
}

#[tauri::command]
pub fn apply_picker_guard(app: AppHandle) -> Result<PickerGuardStatus, String> {
    // User-triggered: always full quit → patch → host-rules relaunch.
    relaunch_guarded_internal(Some(&app))
}

#[tauri::command]
pub fn relaunch_chatgpt_guarded(app: AppHandle) -> Result<PickerGuardStatus, String> {
    relaunch_guarded_internal(Some(&app))
}

#[tauri::command]
pub fn open_chatgpt_guarded(_app: AppHandle) -> Result<PickerGuardStatus, String> {
    if chatgpt_running() && !chatgpt_host_rules_active() {
        // Must restart to attach host-rules.
        quit_chatgpt()?;
        std::thread::sleep(Duration::from_millis(400));
    } else if chatgpt_running() && chatgpt_host_rules_active() {
        let cfg = load_config(Some(&_app));
        return Ok(build_status(Some(&_app), &cfg));
    }
    open_chatgpt();
    std::thread::sleep(Duration::from_millis(800));
    let cfg = load_config(Some(&_app));
    Ok(build_status(Some(&_app), &cfg))
}

#[tauri::command]
pub fn set_picker_guard_enabled(
    app: AppHandle,
    enabled: bool,
) -> Result<PickerGuardStatus, String> {
    let mut cfg = load_config(Some(&app));
    cfg.enabled = enabled;
    save_config(Some(&app), &cfg)?;
    if enabled {
        let _ = apply_internal(Some(&app), cfg.force_quit, cfg.force_quit);
        let cfg = load_config(Some(&app));
        Ok(build_status(Some(&app), &cfg))
    } else {
        RUNTIME.lock().pending_fix = false;
        Ok(build_status(Some(&app), &cfg))
    }
}
