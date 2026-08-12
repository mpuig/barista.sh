//! The node's Contract A parse-and-validate (barista-033). An `InstanceSpec`
//! arrives from a loopback client and, before anything is journaled, passes
//! through `admission::admit` — the check that sits below both entrances. This
//! decodes arbitrary bytes as a spec and, when they parse, admits them: it must
//! reject any spec cleanly (a `Refusal`) and never panic, whatever the id, digest,
//! or egress policy an attacker-influenced record carries.

#![no_main]

use barista_node_agent::admission::admit;
use barista_proto::node::v1alpha1 as pb;
use libfuzzer_sys::fuzz_target;
use prost::Message;

fuzz_target!(|data: &[u8]| {
    if let Ok(spec) = pb::InstanceSpec::decode(data) {
        let caps = pb::RuntimeCapabilities::default();
        // Both isolation demands, so the capability-gated branches are reached.
        let _ = admit(&spec, false, &caps, "fuzz");
        let _ = admit(&spec, true, &caps, "fuzz");
    }
});
