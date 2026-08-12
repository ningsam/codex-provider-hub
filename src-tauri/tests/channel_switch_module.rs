#![allow(dead_code)]

// Keep F2 independently testable until the parallel feature branches register
// its commands in `src/lib.rs`.
#[path = "../src/channel_switch.rs"]
mod channel_switch;
