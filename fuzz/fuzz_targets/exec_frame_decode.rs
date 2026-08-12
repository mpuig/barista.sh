//! The guest's exec frame parse (barista-033). A client streams `ExecFrame`s to
//! the guest; this drives their decode on arbitrary bytes. Decode only —
//! deliberately not `exec::serve`, which spawns the workload process, so a fuzzer
//! must never reach a path that would execute a fuzzer-chosen command. The
//! hostile-frame *handling* is pinned deterministically in `exec.rs`'s tests.

#![no_main]

use barista_proto::guest::v1alpha1 as pb;
use libfuzzer_sys::fuzz_target;
use prost::Message;

fuzz_target!(|data: &[u8]| {
    let _ = pb::ExecFrame::decode(data);
    let _ = pb::ExecStart::decode(data);
});
