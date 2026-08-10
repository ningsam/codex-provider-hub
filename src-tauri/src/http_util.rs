//! Shared HTTP helpers (timeouts, UA, short TTL cache).

use once_cell::sync::Lazy;
use parking_lot::Mutex;
use reqwest::blocking::Client;
use std::collections::HashMap;
use std::time::{Duration, Instant};

pub const HTTP_TIMEOUT: Duration = Duration::from_secs(8);
pub const BROWSER_UA: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
     (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

pub static HTTP: Lazy<Client> = Lazy::new(|| {
    Client::builder()
        .timeout(HTTP_TIMEOUT)
        .user_agent(BROWSER_UA)
        .build()
        .expect("build reqwest client")
});

struct CacheEntry {
    expires_at: Instant,
    payload: String,
}

static CACHE: Lazy<Mutex<HashMap<String, CacheEntry>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

/// Return cached JSON value or compute via `fetch`, storing for `ttl`.
pub fn cached_json<T, F>(key: &str, ttl: Duration, fetch: F) -> Result<T, String>
where
    T: serde::de::DeserializeOwned + serde::Serialize,
    F: FnOnce() -> Result<T, String>,
{
    {
        let guard = CACHE.lock();
        if let Some(entry) = guard.get(key) {
            if entry.expires_at > Instant::now() {
                if let Ok(v) = serde_json::from_str::<T>(&entry.payload) {
                    return Ok(v);
                }
            }
        }
    }

    let value = fetch()?;
    let payload = serde_json::to_string(&value).map_err(|e| format!("cache serialize: {e}"))?;
    CACHE.lock().insert(
        key.to_string(),
        CacheEntry {
            expires_at: Instant::now() + ttl,
            payload,
        },
    );
    Ok(value)
}

pub fn invalidate_cache(key: &str) {
    CACHE.lock().remove(key);
}

pub fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339()
}

pub fn friendly_http_err(context: &str, err: reqwest::Error) -> String {
    if err.is_timeout() {
        format!("{context}: 请求超时")
    } else if err.is_connect() {
        format!("{context}: 无法连接")
    } else {
        format!("{context}: {err}")
    }
}
