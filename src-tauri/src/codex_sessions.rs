//! Unified browser for Codex desktop sessions stored in `~/.codex/state_5.sqlite`.
//!
//! Listing always uses SQLite's read-only open flag. The only mutation exposed
//! by this module first refuses to run while Codex/ChatGPT or app-server is
//! active, creates a consistent SQLite backup (including WAL contents), and
//! then updates `threads.model_provider` in one transaction.

use chrono::Utc;
use rusqlite::{Connection, DatabaseName, OpenFlags, TransactionBehavior};
use serde::Serialize;
use std::{
    collections::HashSet,
    fs::{self, OpenOptions},
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

const THREAD_COLUMNS: &[&str] = &[
    "id",
    "cwd",
    "title",
    "updated_at",
    "model_provider",
    "tokens_used",
    "archived",
];

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CodexThreadSummary {
    pub id: String,
    pub cwd: String,
    pub title: String,
    /// Unix timestamp in seconds, matching `threads.updated_at`.
    pub updated_at: i64,
    pub model_provider: String,
    pub tokens_used: i64,
    pub archived: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CodexSessionsSnapshot {
    pub threads: Vec<CodexThreadSummary>,
    pub total_count: u64,
    pub archived_count: u64,
    pub current_provider: Option<String>,
    pub database_path: String,
    pub codex_running: bool,
    pub blocking_processes: Vec<String>,
    pub merge_ready: bool,
    pub merge_blocked_reason: Option<String>,
    pub fetched_at: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CodexSessionsMergeResult {
    pub current_provider: String,
    pub updated_count: u64,
    pub total_count: u64,
    pub backup_path: Option<String>,
    pub message: String,
}

fn codex_dir() -> Result<PathBuf, String> {
    dirs::home_dir()
        .map(|home| home.join(".codex"))
        .ok_or_else(|| "无法确定用户主目录，不能定位 ~/.codex".to_string())
}

fn state_db_path() -> Result<PathBuf, String> {
    Ok(codex_dir()?.join("state_5.sqlite"))
}

fn config_path() -> Result<PathBuf, String> {
    Ok(codex_dir()?.join("config.toml"))
}

fn ensure_regular_file(path: &Path, label: &str) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|e| format!("无法读取{label} {}：{e}", path.display()))?;
    if metadata.file_type().is_symlink() {
        return Err(format!("拒绝操作符号链接形式的{label}：{}", path.display()));
    }
    if !metadata.is_file() {
        return Err(format!("{label}不是普通文件：{}", path.display()));
    }
    Ok(())
}

fn open_read_only(path: &Path) -> Result<Connection, String> {
    ensure_regular_file(path, "Codex 会话数据库")?;
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| format!("只读打开 Codex 会话数据库失败：{e}"))?;
    connection
        .busy_timeout(Duration::from_secs(2))
        .map_err(|e| format!("设置 SQLite 只读超时失败：{e}"))?;
    connection
        .execute_batch("PRAGMA query_only = ON;")
        .map_err(|e| format!("启用 SQLite 只读保护失败：{e}"))?;
    Ok(connection)
}

fn validate_threads_schema(connection: &Connection) -> Result<(), String> {
    let mut statement = connection
        .prepare("PRAGMA table_info(threads)")
        .map_err(|e| format!("读取 threads schema 失败：{e}"))?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|e| format!("读取 threads schema 失败：{e}"))?
        .collect::<Result<HashSet<_>, _>>()
        .map_err(|e| format!("解析 threads schema 失败：{e}"))?;

    if columns.is_empty() {
        return Err("Codex 会话数据库中不存在 threads 表".to_string());
    }

    let missing = THREAD_COLUMNS
        .iter()
        .filter(|column| !columns.contains(**column))
        .copied()
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(format!(
            "Codex threads schema 不兼容，缺少字段：{}",
            missing.join(", ")
        ));
    }
    Ok(())
}

fn read_threads(connection: &Connection) -> Result<Vec<CodexThreadSummary>, String> {
    validate_threads_schema(connection)?;
    let mut statement = connection
        .prepare(
            "SELECT id, cwd, title, updated_at, model_provider, tokens_used, archived \
             FROM threads ORDER BY updated_at DESC, id DESC",
        )
        .map_err(|e| format!("准备会话查询失败：{e}"))?;
    let threads = statement
        .query_map([], |row| {
            Ok(CodexThreadSummary {
                id: row.get(0)?,
                cwd: row.get(1)?,
                title: row.get(2)?,
                updated_at: row.get(3)?,
                model_provider: row.get(4)?,
                tokens_used: row.get(5)?,
                archived: row.get::<_, i64>(6)? != 0,
            })
        })
        .map_err(|e| format!("查询 Codex 会话失败：{e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 Codex 会话失败：{e}"))?;
    Ok(threads)
}

fn validate_provider(provider: &str) -> Result<String, String> {
    let provider = provider.trim();
    if provider.is_empty()
        || provider.len() > 128
        || !provider
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
    {
        return Err(format!(
            "config.toml 中的 model_provider 不合法：{provider:?}"
        ));
    }
    Ok(provider.to_string())
}

fn read_current_provider(path: &Path) -> Result<String, String> {
    ensure_regular_file(path, "Codex 配置文件")?;
    let raw = fs::read_to_string(path)
        .map_err(|e| format!("读取 Codex 配置 {} 失败：{e}", path.display()))?;
    let config: toml::Value =
        toml::from_str(&raw).map_err(|e| format!("解析 Codex config.toml 失败：{e}"))?;
    let provider = config
        .get("model_provider")
        .and_then(toml::Value::as_str)
        .ok_or_else(|| "Codex config.toml 缺少顶层 model_provider".to_string())?;
    validate_provider(provider)
}

fn blocking_processes_from_ps(output: &str) -> Vec<String> {
    let mut chatgpt = false;
    let mut codex = false;
    let mut app_server = false;

    for line in output.lines() {
        let normalized = line.trim();
        if normalized.contains("/ChatGPT.app/Contents/MacOS/ChatGPT") {
            chatgpt = true;
        }
        if normalized.contains("/Codex.app/Contents/MacOS/Codex") {
            codex = true;
        }
        if normalized
            .split_ascii_whitespace()
            .any(|part| part == "app-server")
        {
            app_server = true;
        }
    }

    let mut result = Vec::new();
    if chatgpt {
        result.push("ChatGPT.app".to_string());
    }
    if codex {
        result.push("Codex.app".to_string());
    }
    if app_server {
        result.push("app-server".to_string());
    }
    result
}

fn detect_blocking_processes() -> Result<Vec<String>, String> {
    let output = Command::new("/bin/ps")
        .args(["-axo", "pid=,comm=,args="])
        .output()
        .map_err(|e| format!("无法运行 /bin/ps 检测 Codex 进程：{e}"))?;
    if !output.status.success() {
        return Err(format!(
            "/bin/ps 检测 Codex 进程失败（状态 {}）",
            output.status
        ));
    }
    let stdout = String::from_utf8(output.stdout)
        .map_err(|_| "/bin/ps 返回了无法解析的进程列表".to_string())?;
    Ok(blocking_processes_from_ps(&stdout))
}

fn stopped_guard(processes: &[String]) -> Result<(), String> {
    if processes.is_empty() {
        return Ok(());
    }
    Err(format!(
        "检测到 {} 正在运行。请完全退出 Codex/ChatGPT 后重试；会话数据库未修改。",
        processes.join("、")
    ))
}

fn next_backup_path(source: &Path) -> Result<PathBuf, String> {
    let parent = source
        .parent()
        .ok_or_else(|| "Codex 会话数据库路径没有父目录".to_string())?;
    let source_name = source
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "Codex 会话数据库文件名无效".to_string())?;
    let stamp = Utc::now().format("%Y%m%dT%H%M%S%.3fZ");

    for suffix in 0..100_u8 {
        let name = if suffix == 0 {
            format!("{source_name}.bak-{stamp}")
        } else {
            format!("{source_name}.bak-{stamp}-{suffix}")
        };
        let candidate = parent.join(name);
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err("无法生成唯一的 Codex 会话数据库备份名".to_string())
}

fn create_consistent_backup(source: &Path, destination: &Path) -> Result<(), String> {
    ensure_regular_file(source, "Codex 会话数据库")?;
    if source.parent() != destination.parent() {
        return Err("安全检查失败：备份必须与 Codex 会话数据库位于同一目录".to_string());
    }
    let source_name = source
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "Codex 会话数据库文件名无效".to_string())?;
    let destination_name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "Codex 会话数据库备份文件名无效".to_string())?;
    if !destination_name.starts_with(&format!("{source_name}.bak-")) {
        return Err("安全检查失败：Codex 会话数据库备份名无效".to_string());
    }

    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    options
        .open(destination)
        .map_err(|e| format!("创建备份 {} 失败：{e}", destination.display()))?;

    let result = (|| {
        let source_connection = open_read_only(source)?;
        validate_threads_schema(&source_connection)?;
        source_connection
            .backup(DatabaseName::Main, destination, None)
            .map_err(|e| format!("创建 SQLite 一致性备份失败：{e}"))?;

        let backup_connection = open_read_only(destination)?;
        let integrity: String = backup_connection
            .query_row("PRAGMA integrity_check(1)", [], |row| row.get(0))
            .map_err(|e| format!("校验 SQLite 备份失败：{e}"))?;
        if integrity != "ok" {
            return Err(format!("SQLite 备份完整性校验失败：{integrity}"));
        }
        Ok(())
    })();

    if result.is_err() {
        let _ = fs::remove_file(destination);
    }
    result
}

fn list_sessions_at(
    database_path: &Path,
    codex_config_path: &Path,
) -> Result<CodexSessionsSnapshot, String> {
    let connection = open_read_only(database_path)?;
    let threads = read_threads(&connection)?;
    let archived_count = threads.iter().filter(|thread| thread.archived).count() as u64;

    let provider_result = read_current_provider(codex_config_path);
    let process_result = detect_blocking_processes();
    let (blocking_processes, process_error) = match process_result {
        Ok(processes) => (processes, None),
        Err(error) => (Vec::new(), Some(error)),
    };
    let codex_running = !blocking_processes.is_empty();

    let current_provider = provider_result.as_ref().ok().cloned();
    let merge_blocked_reason = if let Some(error) = process_error {
        Some(error)
    } else if codex_running {
        Some(format!(
            "请先完全退出 {}（包括 app-server），再合并会话分区。",
            blocking_processes.join("、")
        ))
    } else {
        provider_result.as_ref().err().cloned()
    };

    Ok(CodexSessionsSnapshot {
        total_count: threads.len() as u64,
        archived_count,
        threads,
        current_provider,
        database_path: database_path.display().to_string(),
        codex_running,
        blocking_processes,
        merge_ready: merge_blocked_reason.is_none(),
        merge_blocked_reason,
        fetched_at: Utc::now().to_rfc3339(),
    })
}

fn merge_sessions_at_with_guard<F>(
    database_path: &Path,
    codex_config_path: &Path,
    mut process_probe: F,
) -> Result<CodexSessionsMergeResult, String>
where
    F: FnMut() -> Result<Vec<String>, String>,
{
    let provider = read_current_provider(codex_config_path)?;
    stopped_guard(&process_probe()?)?;

    let read_connection = open_read_only(database_path)?;
    validate_threads_schema(&read_connection)?;
    let total_count: i64 = read_connection
        .query_row("SELECT COUNT(*) FROM threads", [], |row| row.get(0))
        .map_err(|e| format!("统计 Codex 会话失败：{e}"))?;
    let pending_count: i64 = read_connection
        .query_row(
            "SELECT COUNT(*) FROM threads WHERE model_provider <> ?1",
            [&provider],
            |row| row.get(0),
        )
        .map_err(|e| format!("统计待合并 Codex 会话失败：{e}"))?;
    drop(read_connection);

    if pending_count == 0 {
        return Ok(CodexSessionsMergeResult {
            current_provider: provider.clone(),
            updated_count: 0,
            total_count: total_count as u64,
            backup_path: None,
            message: format!("全部 {total_count} 条会话已在 {provider} 分区，无需修改。"),
        });
    }

    let backup_path = next_backup_path(database_path)?;
    create_consistent_backup(database_path, &backup_path)?;

    if let Err(error) = stopped_guard(&process_probe()?) {
        return Err(format!(
            "{error} 已创建安全备份 {}，但没有写入原数据库。",
            backup_path.display()
        ));
    }

    ensure_regular_file(database_path, "Codex 会话数据库")?;
    let mut connection = Connection::open_with_flags(
        database_path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| format!("以事务模式打开 Codex 会话数据库失败：{e}"))?;
    connection
        .busy_timeout(Duration::from_secs(3))
        .map_err(|e| format!("设置 SQLite 写入超时失败：{e}"))?;
    validate_threads_schema(&connection)?;

    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|e| format!("启动 Codex 会话合并事务失败：{e}"))?;
    if let Err(error) = stopped_guard(&process_probe()?) {
        return Err(format!(
            "{error} 已创建安全备份 {}；事务未写入并已回滚。",
            backup_path.display()
        ));
    }
    let provider_after_backup = read_current_provider(codex_config_path)?;
    if provider_after_backup != provider {
        return Err(format!(
            "检测到 Codex config.toml 在合并期间从 {provider} 变为 {provider_after_backup}；事务未写入并已回滚。备份位于 {}",
            backup_path.display()
        ));
    }
    let updated = transaction
        .execute(
            "UPDATE threads SET model_provider = ?1 WHERE model_provider <> ?1",
            [&provider],
        )
        .map_err(|e| format!("合并 Codex 会话分区失败：{e}"))?;
    let remaining: i64 = transaction
        .query_row(
            "SELECT COUNT(*) FROM threads WHERE model_provider <> ?1",
            [&provider],
            |row| row.get(0),
        )
        .map_err(|e| format!("校验 Codex 会话合并结果失败：{e}"))?;
    if remaining != 0 {
        return Err(format!(
            "合并结果校验失败：仍有 {remaining} 条会话不在 {provider} 分区；事务已回滚"
        ));
    }
    transaction
        .commit()
        .map_err(|e| format!("提交 Codex 会话合并事务失败：{e}"))?;

    Ok(CodexSessionsMergeResult {
        current_provider: provider.clone(),
        updated_count: updated as u64,
        total_count: total_count as u64,
        backup_path: Some(backup_path.display().to_string()),
        message: format!(
            "已将 {updated} 条会话并入 {provider} 分区。重启 Codex 后即可看到完整历史。"
        ),
    })
}

/// Read all Codex threads without filtering by provider.
#[tauri::command]
pub fn list_codex_sessions() -> Result<CodexSessionsSnapshot, String> {
    list_sessions_at(&state_db_path()?, &config_path()?)
}

/// Move every thread into the provider currently selected in config.toml.
#[tauri::command]
pub fn merge_codex_sessions_into_current_provider() -> Result<CodexSessionsMergeResult, String> {
    merge_sessions_at_with_guard(
        &state_db_path()?,
        &config_path()?,
        detect_blocking_processes,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "codex-provider-hub-sessions-test-{}",
                Uuid::new_v4()
            ));
            fs::create_dir_all(&path).expect("create test directory");
            Self(path)
        }

        fn join(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn create_test_database(path: &Path) -> Connection {
        let connection = Connection::open(path).expect("open test database");
        connection
            .execute_batch(
                "CREATE TABLE threads (
                    id TEXT PRIMARY KEY,
                    cwd TEXT NOT NULL,
                    title TEXT NOT NULL,
                    updated_at INTEGER NOT NULL,
                    model_provider TEXT NOT NULL,
                    tokens_used INTEGER NOT NULL DEFAULT 0,
                    archived INTEGER NOT NULL DEFAULT 0
                );",
            )
            .expect("create threads table");
        connection
    }

    fn write_config(path: &Path, provider: &str) {
        fs::write(path, format!("model_provider = \"{provider}\"\n")).expect("write config");
    }

    #[test]
    fn lists_complete_thread_schema_read_only() {
        let dir = TestDir::new();
        let database = dir.join("state_5.sqlite");
        let config = dir.join("config.toml");
        let connection = create_test_database(&database);
        connection
            .execute(
                "INSERT INTO threads VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                (
                    "thread-1",
                    "/tmp/project",
                    "Visible title",
                    123_i64,
                    "sub2api",
                    456_i64,
                    1_i64,
                ),
            )
            .expect("insert thread");
        drop(connection);
        write_config(&config, "openai");

        let connection = open_read_only(&database).expect("read-only open");
        let threads = read_threads(&connection).expect("read threads");
        assert_eq!(
            threads,
            vec![CodexThreadSummary {
                id: "thread-1".into(),
                cwd: "/tmp/project".into(),
                title: "Visible title".into(),
                updated_at: 123,
                model_provider: "sub2api".into(),
                tokens_used: 456,
                archived: true,
            }]
        );
        assert!(connection
            .execute("DELETE FROM threads", [])
            .expect_err("read-only connection must reject writes")
            .to_string()
            .contains("readonly"));
    }

    #[test]
    fn reports_missing_schema_columns() {
        let dir = TestDir::new();
        let database = dir.join("state_5.sqlite");
        let connection = Connection::open(&database).expect("open database");
        connection
            .execute_batch("CREATE TABLE threads (id TEXT PRIMARY KEY, title TEXT NOT NULL);")
            .expect("create incomplete schema");
        drop(connection);

        let connection = open_read_only(&database).expect("read-only open");
        let error = validate_threads_schema(&connection).expect_err("schema must fail");
        assert!(error.contains("cwd"));
        assert!(error.contains("model_provider"));
    }

    #[test]
    fn backup_includes_committed_wal_content_and_is_valid() {
        let dir = TestDir::new();
        let database = dir.join("state_5.sqlite");
        let backup = dir.join("state_5.sqlite.bak-test");
        let connection = create_test_database(&database);
        connection
            .pragma_update(None, "journal_mode", "WAL")
            .expect("enable WAL");
        connection
            .execute(
                "INSERT INTO threads VALUES ('wal-thread', '/tmp', 'WAL', 1, 'openai', 10, 0)",
                [],
            )
            .expect("insert WAL row");

        create_consistent_backup(&database, &backup).expect("create backup");
        let backup_connection = open_read_only(&backup).expect("open backup");
        let count: i64 = backup_connection
            .query_row(
                "SELECT COUNT(*) FROM threads WHERE id = 'wal-thread'",
                [],
                |row| row.get(0),
            )
            .expect("query backup");
        assert_eq!(count, 1);
    }

    #[test]
    fn process_guard_detects_apps_and_app_server_without_false_shell_match() {
        let output = r#"
          101 /Applications/ChatGPT /Applications/ChatGPT.app/Contents/MacOS/ChatGPT
          102 /Applications/codex /Applications/ChatGPT.app/Contents/Resources/codex app-server --analytics
          103 /bin/zsh /bin/zsh -lc rg -i 'ChatGPT.app|app-server'
          104 /Applications/Hub /Applications/Codex Provider Hub.app/Contents/MacOS/hub
          105 /Applications/Codex /Applications/Codex.app/Contents/MacOS/Codex
        "#;
        assert_eq!(
            blocking_processes_from_ps(output),
            vec!["ChatGPT.app", "Codex.app", "app-server"]
        );
        assert!(blocking_processes_from_ps("42 /bin/zsh /bin/zsh -lc app-server-check").is_empty());
    }

    #[test]
    fn merge_guard_refuses_before_backup_or_write() {
        let dir = TestDir::new();
        let database = dir.join("state_5.sqlite");
        let config = dir.join("config.toml");
        let connection = create_test_database(&database);
        connection
            .execute(
                "INSERT INTO threads VALUES ('t', '/tmp', 'Title', 1, 'sub2api', 0, 0)",
                [],
            )
            .expect("insert thread");
        drop(connection);
        write_config(&config, "openai");

        let error =
            merge_sessions_at_with_guard(&database, &config, || Ok(vec!["app-server".to_string()]))
                .expect_err("running process must block merge");
        assert!(error.contains("app-server"));

        let connection = open_read_only(&database).expect("open original");
        let provider: String = connection
            .query_row("SELECT model_provider FROM threads", [], |row| row.get(0))
            .expect("read provider");
        assert_eq!(provider, "sub2api");
        assert_eq!(
            fs::read_dir(&dir.0)
                .expect("read test dir")
                .filter_map(Result::ok)
                .filter(|entry| entry.file_name().to_string_lossy().contains(".bak-"))
                .count(),
            0
        );
    }

    #[test]
    fn merge_guard_rechecks_inside_transaction_before_writing() {
        let dir = TestDir::new();
        let database = dir.join("state_5.sqlite");
        let config = dir.join("config.toml");
        let connection = create_test_database(&database);
        connection
            .execute(
                "INSERT INTO threads VALUES ('t', '/tmp', 'Title', 1, 'sub2api', 0, 0)",
                [],
            )
            .expect("insert thread");
        drop(connection);
        write_config(&config, "openai");

        let probe_count = std::cell::Cell::new(0_u8);
        let error = merge_sessions_at_with_guard(&database, &config, || {
            let current = probe_count.get();
            probe_count.set(current + 1);
            Ok(if current >= 2 {
                vec!["app-server".to_string()]
            } else {
                Vec::new()
            })
        })
        .expect_err("process starting after backup must block the write");
        assert!(error.contains("事务未写入并已回滚"));

        let connection = open_read_only(&database).expect("open original");
        let provider: String = connection
            .query_row("SELECT model_provider FROM threads", [], |row| row.get(0))
            .expect("read provider");
        assert_eq!(provider, "sub2api");
        assert_eq!(
            fs::read_dir(&dir.0)
                .expect("read test dir")
                .filter_map(Result::ok)
                .filter(|entry| entry.file_name().to_string_lossy().contains(".bak-"))
                .count(),
            1
        );
    }

    #[test]
    fn merge_backs_up_then_updates_in_one_transaction() {
        let dir = TestDir::new();
        let database = dir.join("state_5.sqlite");
        let config = dir.join("config.toml");
        let connection = create_test_database(&database);
        connection
            .execute_batch(
                "INSERT INTO threads VALUES ('a', '/one', 'One', 2, 'sub2api', 1, 0);
                 INSERT INTO threads VALUES ('b', '/two', 'Two', 1, 'openai', 2, 1);",
            )
            .expect("insert threads");
        drop(connection);
        write_config(&config, "openai");

        let result = merge_sessions_at_with_guard(&database, &config, || Ok(Vec::new()))
            .expect("merge sessions");
        assert_eq!(result.updated_count, 1);
        let backup = result.backup_path.expect("backup path");

        let connection = open_read_only(&database).expect("open merged database");
        let providers: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM threads WHERE model_provider <> 'openai'",
                [],
                |row| row.get(0),
            )
            .expect("count providers");
        assert_eq!(providers, 0);

        let backup_connection = open_read_only(Path::new(&backup)).expect("open backup");
        let old_provider: String = backup_connection
            .query_row(
                "SELECT model_provider FROM threads WHERE id = 'a'",
                [],
                |row| row.get(0),
            )
            .expect("read backup provider");
        assert_eq!(old_provider, "sub2api");
    }
}
