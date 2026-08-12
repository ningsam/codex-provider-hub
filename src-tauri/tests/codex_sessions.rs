#![allow(dead_code)]

// Keep F1 independently testable until the command module is registered in
// `src/lib.rs` by the integration pass.
#[path = "../src/codex_sessions.rs"]
mod codex_sessions;
