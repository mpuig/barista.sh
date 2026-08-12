//! The guest's bootstrap parse (barista-033). The substrate hands the guest
//! `base64(prost(Process))` / `...(Hooks)` verbatim in the sandbox environment
//! (`hypeman` returns it from `GET /instances/{id}`), so this is attacker-
//! influenced input into a process that is a live session's PID 1. Its only
//! honest outcome is `Ok`/`Err` — never a panic, an abort, or a hang.

#![no_main]

use barista_guest_agent::bootstrap::decode_value;
use barista_proto::node::v1alpha1 as node;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // The value arrives as a string; the bytes stand in for whatever the
    // substrate delivered. Lossy is fine — the point is the base64+prost parser,
    // not a round trip.
    let value = String::from_utf8_lossy(data);
    let _ = decode_value::<node::Process>(&value);
    let _ = decode_value::<node::Hooks>(&value);
});
