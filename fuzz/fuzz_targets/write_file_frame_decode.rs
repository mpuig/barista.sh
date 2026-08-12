//! The guest's file-RPC request parse (barista-033). `ReadFile`/`WriteFile`
//! carry a client-supplied path and, for writes, a `mode` and a frame stream;
//! this drives the decode of those request messages on arbitrary bytes. Decode
//! only — no path is opened and nothing is written, so the target exercises the
//! parser without touching the filesystem the real handler would.

#![no_main]

use barista_proto::guest::v1alpha1 as pb;
use libfuzzer_sys::fuzz_target;
use prost::Message;

fuzz_target!(|data: &[u8]| {
    let _ = pb::ReadFileRequest::decode(data);
    let _ = pb::WriteFileRequest::decode(data);
});
