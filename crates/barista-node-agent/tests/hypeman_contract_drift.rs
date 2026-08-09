//! Drift guard for the vendored hypeman contract.
//!
//! `progenitor` rejects OpenAPI 3.1 and the vendored document is 3.1.0, so the
//! client in `runtime::hypeman::client` is hand-written (nap-005 design decision
//! 2). This test is what replaces the safety a generator would have given: if an
//! operation or a field the client depends on moves, `make check` fails here
//! instead of the node failing at runtime against a bumped daemon.
//!
//! It asserts on the document as text rather than parsing YAML, because pulling a
//! YAML parser in for one test is a worse trade than a handful of anchored
//! assertions — and the indentation of an OpenAPI document is structural.
//!
//! **This test cannot cover `exec`**, which is a WebSocket endpoint the document
//! does not describe at all. That surface is guarded only by integration tests,
//! and the asymmetry is deliberate and recorded, not an oversight.

use std::path::PathBuf;

fn contract() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../vendor/hypeman/openapi.yaml");
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("vendored contract missing at {}: {e}", path.display()))
}

/// The body of a `components.schemas.<name>` block, up to the next sibling schema.
fn schema_block<'a>(doc: &'a str, schema: &str) -> &'a str {
    let header = format!("\n    {schema}:\n");
    let start = doc
        .find(&header)
        .unwrap_or_else(|| panic!("schema `{schema}` is gone from the vendored contract"))
        + header.len();
    let rest = &doc[start..];
    // The next sibling schema is the next line at exactly four spaces of indent.
    let end = rest
        .split_inclusive('\n')
        .scan(0usize, |offset, line| {
            let at = *offset;
            *offset += line.len();
            Some((at, line))
        })
        .find(|(at, line)| {
            *at > 0
                && line.starts_with("    ")
                && !line.starts_with("     ")
                && line.trim_end().ends_with(':')
        })
        .map(|(at, _)| at)
        .unwrap_or(rest.len());
    &rest[..end]
}

/// Extract the property names of a `components.schemas.<name>` block.
fn schema_properties(doc: &str, schema: &str) -> Vec<String> {
    schema_block(doc, schema)
        .split_inclusive('\n')
        .filter_map(|line| {
            let trimmed = line.trim_end();
            // Properties sit at eight spaces: `        field:`
            (trimmed.starts_with("        ")
                && !trimmed.starts_with("         ")
                && trimmed.ends_with(':'))
            .then(|| trimmed.trim().trim_end_matches(':').to_string())
        })
        .collect()
}

/// Everything indented under `<indent><key>:`, up to the next sibling key.
///
/// The bound is the whole point. Every slice in this file that forgot one ran on
/// into the next block and could be satisfied by a rule belonging to something
/// else — the trap the `tags` deepObject check already documents. Written once
/// here for the assertions that need to reach *inside* a block rather than merely
/// list its keys.
fn block_under(text: &str, indent: usize, key: &str) -> String {
    // Anchored on a newline so `key:` can never match a longer key ending in it,
    // and prefixed here so the *first* key of a block is findable too — a block
    // handed in by a previous call starts at its own first line, with no newline
    // in front of it.
    let text = format!("\n{text}");
    let header = format!("\n{}{key}:\n", " ".repeat(indent));
    let start = text
        .find(&header)
        .unwrap_or_else(|| panic!("`{key}` is gone from the vendored contract"))
        + header.len();
    let rest = &text[start..];
    let end = rest
        .split_inclusive('\n')
        .scan(0usize, |offset, line| {
            let at = *offset;
            *offset += line.len();
            Some((at, line))
        })
        // The next sibling — or anything shallower, which ends the block just as
        // surely. Measured by indentation rather than by "ends with a colon": a
        // response status carries none of the trailing structure a schema key
        // does, and `409:` is a sibling of `201:` all the same.
        .find(|(at, line)| {
            *at > 0
                && !line.trim().is_empty()
                && line.len() - line.trim_start_matches(' ').len() <= indent
        })
        .map(|(at, _)| at)
        .unwrap_or(rest.len());
    rest[..end].to_string()
}

/// One operation's body: `paths./the/path.<method>`.
fn operation(doc: &str, path: &str, method: &str) -> String {
    let path_block = block_under(doc, 2, path);
    block_under(&path_block, 4, method)
}

/// The body of **one property** of a schema, up to the next sibling property.
///
/// Needed because the egress policy lives two levels down (`network.egress`), and
/// a search over the whole schema would not be an assertion about it: the word
/// "egress" appears in `CreateInstanceRequest`'s own `credentials` description
/// ("materialized on the mediated egress path"), so a slice that ran past the
/// `network` property would keep matching long after the field was gone.
///
/// Delegates to [`block_under`] rather than carrying a second copy of the same
/// bounding rule: schema properties sit at eight spaces, which is the only thing
/// this ever knew that the general form does not take as an argument.
fn property_block(doc: &str, schema: &str, property: &str) -> String {
    block_under(schema_block(doc, schema), 8, property)
}

/// The client is written against exactly this API version.
#[test]
fn pinned_api_version_still_matches() {
    let doc = contract();
    let expected = format!(
        "  version: {}",
        barista_node_agent::runtime::hypeman::client::PINNED_API_VERSION
    );
    assert!(
        doc.lines().any(|l| l.trim_end() == expected.trim_end()),
        "vendored contract is no longer API {}; re-read vendor/hypeman/README.md \
         before bumping the pin",
        barista_node_agent::runtime::hypeman::client::PINNED_API_VERSION
    );
    assert!(
        doc.starts_with("openapi: 3.1"),
        "contract is no longer OpenAPI 3.1 — if it became 3.0.x, generation with \
         progenitor is viable again and design decision 2 should be revisited"
    );
}

/// Every operation the client calls must still exist, at the method it uses.
#[test]
fn operations_the_client_calls_still_exist() {
    let doc = contract();
    let required: &[(&str, &str)] = &[
        ("/health", "get"),
        ("/instances", "post"),
        ("/instances", "get"),
        ("/instances/{id}", "get"),
        ("/instances/{id}", "delete"),
        ("/instances/{id}/start", "post"),
        ("/instances/{id}/stop", "post"),
        ("/instances/{id}/standby", "post"),
        ("/instances/{id}/restore", "post"),
        // The explicit-snapshot tier (nap-010): create, restore-by-id, and the
        // reads the journal reconciles against.
        ("/instances/{id}/snapshots", "post"),
        ("/instances/{id}/snapshots/{snapshotId}/restore", "post"),
        ("/snapshots", "get"),
        ("/snapshots/{snapshotId}", "get"),
        ("/snapshots/{snapshotId}", "delete"),
        // Credentials (nap-016): the volume the token rides on, and the listing
        // the reaper sweeps. `GET /volumes` is the one the sweep cannot work
        // without — losing it turns the credential half of the zero-orphan
        // invariant back into the leak it was.
        ("/volumes", "get"),
        ("/volumes/from-archive", "post"),
        ("/volumes/{id}", "get"),
        ("/volumes/{id}", "delete"),
    ];
    for (path, method) in required {
        let header = format!("\n  {path}:\n");
        let start = doc
            .find(&header)
            .unwrap_or_else(|| panic!("path `{path}` is gone from the vendored contract"))
            + header.len();
        let rest = &doc[start..];
        // Methods for this path are the four-space lines before the next path.
        let end = rest.find("\n  /").unwrap_or(rest.len());
        let verbs: Vec<&str> = rest[..end]
            .lines()
            .map(str::trim_end)
            .filter(|l| l.starts_with("    ") && !l.starts_with("     ") && l.ends_with(':'))
            .map(|l| l.trim().trim_end_matches(':'))
            .collect();
        assert!(
            verbs.contains(method),
            "`{method} {path}` is gone; the client calls it. Found only: {verbs:?}"
        );
    }
}

/// Which of those operations demand a request body.
///
/// Added after a real miss: `POST /instances/{id}/start` declares
/// `requestBody: required: true` with only optional properties inside, and the
/// client sent no body at all. Every schema check above passed — the operation
/// existed, at the right method, with the right fields — and the node still
/// failed at runtime with a 400 reading `value is required but missing`, which
/// sounds like a missing field rather than a missing body.
///
/// So the existence checks were not enough: a body-less POST to an operation that
/// requires one is invisible to them. This table is the contract the client is
/// actually written against, and it fails in **both** directions — an operation
/// that starts requiring a body is as breaking as one that stops.
#[test]
fn request_bodies_the_client_sends_match_what_the_contract_demands() {
    let doc = contract();
    // (path, method, does the client send a body?)
    let expected: &[(&str, &str, bool)] = &[
        ("/instances", "post", true),
        ("/instances/{id}/start", "post", true),
        ("/instances/{id}/stop", "post", false),
        ("/instances/{id}/standby", "post", false),
        ("/instances/{id}/restore", "post", false),
        // Both snapshot POSTs require a body. The restore one is the `start`
        // shape again — `required: true` around all-optional properties — and
        // this row is what caught the client sending none (nap-010 task 1.2),
        // this time before the live 400 instead of after it.
        ("/instances/{id}/snapshots", "post", true),
        (
            "/instances/{id}/snapshots/{snapshotId}/restore",
            "post",
            true,
        ),
    ];
    for (path, method, sends_body) in expected {
        let header = format!("\n  {path}:\n");
        let start = doc.find(&header).expect("path checked above") + header.len();
        let rest = &doc[start..];
        let end = rest.find("\n  /").unwrap_or(rest.len());
        let block = &rest[..end];

        // Scope to this method's block: from `    <method>:` to the next verb.
        let verb = format!("    {method}:\n");
        let from = block.find(&verb).expect("method checked above") + verb.len();
        let tail = &block[from..];
        let to = tail
            .split_inclusive('\n')
            .scan(0usize, |offset, line| {
                let at = *offset;
                *offset += line.len();
                Some((at, line))
            })
            .find(|(at, line)| {
                *at > 0
                    && line.starts_with("    ")
                    && !line.starts_with("     ")
                    && line.trim_end().ends_with(':')
            })
            .map(|(at, _)| at)
            .unwrap_or(tail.len());
        let operation = &tail[..to];

        let requires_body = operation.contains("requestBody:")
            && operation
                .split_inclusive('\n')
                .skip_while(|l| !l.contains("requestBody:"))
                .take(3)
                .any(|l| l.trim() == "required: true");

        assert_eq!(
            requires_body,
            *sends_body,
            "`{method} {path}`: the contract {} a request body, the client {}. \
             A mismatch here is a 400 at runtime, not a compile error.",
            if requires_body {
                "requires"
            } else {
                "does not require"
            },
            if *sends_body {
                "sends one"
            } else {
                "sends none"
            },
        );
    }
}

/// Every field the client reads off a response, or sends in a request, must still
/// be declared. Additions upstream are fine — serde ignores them — so this only
/// catches removals and renames, which are exactly the breaking ones.
#[test]
fn fields_the_client_depends_on_still_exist() {
    let doc = contract();

    let instance = schema_properties(&doc, "Instance");
    for field in [
        "id",
        "name",
        "image",
        "state",
        "state_error",
        "hypervisor",
        "tags",
        "exit_code",
        "has_snapshot",
    ] {
        assert!(
            instance.contains(&field.to_string()),
            "`Instance.{field}` is gone; the client deserializes it. Present: {instance:?}"
        );
    }

    let create = schema_properties(&doc, "CreateInstanceRequest");
    for field in [
        "name",
        "image",
        "size",
        "vcpus",
        "hypervisor",
        "env",
        "tags",
        "entrypoint",
        "cmd",
        // Egress policy (nap-014) travels inside `network`; the object it nests
        // is pinned separately below.
        "network",
    ] {
        assert!(
            create.contains(&field.to_string()),
            "`CreateInstanceRequest.{field}` is gone; the client sends it. Present: {create:?}"
        );
    }

    let health = schema_properties(&doc, "Health");
    assert!(
        health.contains(&"status".to_string()),
        "`Health.status` is gone; the preflight reads it. Present: {health:?}"
    );

    // Explicit snapshots (nap-010), plus the name nap-015's `CreateSnapshot`
    // sends, and the fields the client reads back off a `Snapshot`.
    let create_snapshot = schema_properties(&doc, "CreateSnapshotRequest");
    for field in ["kind", "name"] {
        assert!(
            create_snapshot.contains(&field.to_string()),
            "`CreateSnapshotRequest.{field}` is gone; the client sends it. \
             Present: {create_snapshot:?}"
        );
    }
    let snapshot = schema_properties(&doc, "Snapshot");
    for field in ["id", "kind", "source_instance_name", "size_bytes"] {
        assert!(
            snapshot.contains(&field.to_string()),
            "`Snapshot.{field}` is gone; the client deserializes it. Present: {snapshot:?}"
        );
    }

    // Credentials (nap-016). `tags` is what makes a token volume node-owned, and
    // the sweep's every verdict is read off it: without the field the reaper
    // cannot tell its own credentials from a peer node's and must delete nothing.
    let volume = schema_properties(&doc, "Volume");
    for field in ["id", "name", "tags"] {
        assert!(
            volume.contains(&field.to_string()),
            "`Volume.{field}` is gone; the credential sweep reads it. Present: {volume:?}"
        );
    }
}

/// The egress policy nap-014 declares, pinned where the client actually sends it.
///
/// Presence rather than requiredness, deliberately (nap-014 design decision 5).
/// The `network` object is optional on create, so a field that upstream renames
/// or moves does **not** come back as a 400: the substrate accepts the request
/// and silently ignores the policy, and the sandbox boots with exactly the open
/// outbound network the caller asked it not to have. Nothing on Barista's side can
/// observe that — the create succeeded, the instance is `RUNNING`, the capability
/// still reads `egress_control: true` — which is why it has to fail here, at
/// build time, instead.
///
/// Every level is asserted separately because they fail differently: a missing
/// `network` is a compile-time-visible client change, a missing `egress` is the
/// silent-ignore case above, and a renamed **mode literal** is a 400 on every
/// mediated create but on none of the unmediated ones.
#[test]
fn the_egress_policy_the_client_sends_is_still_where_it_sends_it() {
    let doc = contract();

    // `network.egress`, scoped to the `network` property's own block — see
    // `property_block` for why the scope is the assertion.
    let network = property_block(&doc, "CreateInstanceRequest", "network");
    assert!(
        network.contains("CreateInstanceRequestNetworkEgress"),
        "`CreateInstanceRequest.network` no longer references the egress object; \
         a mediated spec would be accepted and ignored. Block was:\n{network}"
    );

    let egress = schema_properties(&doc, "CreateInstanceRequestNetworkEgress");
    for field in ["enabled", "enforcement"] {
        assert!(
            egress.contains(&field.to_string()),
            "`network.egress.{field}` is gone; the client sends it. Present: {egress:?}"
        );
    }

    let enforcement = schema_properties(&doc, "CreateInstanceRequestNetworkEgressEnforcement");
    assert!(
        enforcement.contains(&"mode".to_string()),
        "`network.egress.enforcement.mode` is gone; the client sends it. \
         Present: {enforcement:?}"
    );

    // The literals themselves. `client::EgressMode` serializes to exactly these
    // two strings, so a rename upstream is a 400 the moment any consumer asks for
    // mediation — and only then, which is the worst time to find out.
    let block = schema_block(&doc, "CreateInstanceRequestNetworkEgressEnforcement");
    assert!(
        block.contains("enum: [all, http_https_only]"),
        "the enforcement modes are no longer `all` / `http_https_only`; \
         `client::EgressMode` serializes to those two spellings. Block was:\n{block}"
    );
}

/// The tag filters the node-scoped sweeps send, in the one spelling that works.
///
/// `style: deepObject` is the whole assertion. A `tags=k%3Dv` form is accepted
/// and **silently ignored** by this API — which is how the instance filter once
/// appeared to work while returning every instance on the host, including other
/// nodes' (client.rs `list_instances`). For volumes the same mistake would be
/// worse than a wrong listing: the sweep deletes what it lists, so an ignored
/// filter would point a delete loop at every node's credentials at once.
#[test]
fn the_tag_filters_the_sweeps_depend_on_are_still_deep_objects() {
    let doc = contract();
    for (path, method) in [
        ("/instances", "get"),
        ("/volumes", "get"),
        ("/volumes/from-archive", "post"),
    ] {
        let header = format!("\n  {path}:\n");
        let start = doc.find(&header).expect("path checked above") + header.len();
        let rest = &doc[start..];
        let end = rest.find("\n  /").unwrap_or(rest.len());
        let block = &rest[..end];

        let verb = format!("    {method}:\n");
        let from = block.find(&verb).expect("method checked above") + verb.len();
        let tail = &block[from..];
        let to = tail
            .split_inclusive('\n')
            .scan(0usize, |offset, line| {
                let at = *offset;
                *offset += line.len();
                Some((at, line))
            })
            .find(|(at, line)| {
                *at > 0
                    && line.starts_with("    ")
                    && !line.starts_with("     ")
                    && line.trim_end().ends_with(':')
            })
            .map(|(at, _)| at)
            .unwrap_or(tail.len());
        let operation = &tail[..to];

        let tags_at = operation.find("- name: tags").unwrap_or_else(|| {
            panic!("`{method} {path}` no longer takes a `tags` parameter; the sweep filters on it")
        });
        // Cut at the next parameter *or* at the end of the parameter list.
        // `tags` is the last parameter of `GET /volumes`, so without the second
        // bound the slice would run on into `responses:` and the assertion could
        // be satisfied by a `deepObject` belonging to something else entirely.
        let parameter = &operation[tags_at..];
        let parameter = &parameter[..parameter
            .find("\n        - name:")
            .into_iter()
            .chain(parameter.find("\n      responses:"))
            .min()
            .unwrap_or(parameter.len())];
        assert!(
            parameter.contains("style: deepObject"),
            "`{method} {path}` still takes `tags`, but no longer as a deepObject. \
             The client's `tags[key]=value` spelling would be silently ignored — \
             for /volumes that means a sweep listing every node's credentials"
        );
    }
}

/// The state names the client maps onto Barista's state machine.
///
/// `Standby` is the load-bearing one: Barista's `PAUSED` means "holds zero sandbox
/// resources", which is `Standby` and **not** `Paused` (a Cloud-Hypervisor-native
/// in-memory pause that keeps the VM resident). If `Standby` ever disappears,
/// mapping `PAUSED` onto `Paused` would silently keep every idle session resident.
#[test]
fn instance_states_the_client_maps_still_exist() {
    let doc = contract();
    let line = doc
        .lines()
        .find(|l| l.trim_start().starts_with("enum: [Created"))
        .expect("InstanceState enum is gone from the vendored contract");
    for state in [
        "Created",
        "Initializing",
        "Running",
        "Paused",
        "Shutdown",
        "Stopped",
        "Standby",
        "Unknown",
    ] {
        assert!(
            line.contains(state),
            "`InstanceState::{state}` is gone; the client models it. Found: {line}"
        );
    }
}

/// nap-015 — the two clauses of `createInstanceSnapshot` that Barista **mirrors**
/// rather than merely calls, and which a shape check therefore cannot see.
///
/// 1. The grammar of `name`. `service::is_legal_snapshot_name` is that regex
///    written out longhand, so a caller can be told which rule it broke before
///    the operation enters `CHECKPOINTING`. A mirror that silently stops matching
///    is worse than no mirror: it refuses names the substrate would have taken,
///    or forwards ones it will not.
/// 2. The `409`. It is how a duplicate name arrives from a substrate Barista's own
///    journal cannot see — a peer node, or an artifact created outside Barista — and
///    the runtime maps it to `SNAPSHOT_NAME_CONFLICT`. If it stops being a
///    documented outcome, that mapping is guessing.
///
/// Both are scoped to the block they belong to. `pattern:`, `maxLength: 63` and
/// `409:` all appear dozens of times in this document, so an unbounded search
/// would be satisfied by somebody else's rule and this test would pass for the
/// wrong reason.
#[test]
fn the_snapshot_name_rules_the_node_mirrors_are_still_the_contracts() {
    let doc = contract();

    let schema = block_under(&doc, 4, "CreateSnapshotRequest");
    let properties = block_under(&schema, 6, "properties");
    let name = block_under(&properties, 8, "name");
    // The bound, checked before the rules it bounds: `tags` is `name`'s next
    // sibling, so a slice that swallowed it would happily match a `pattern:`
    // belonging to something else.
    assert!(
        !name.contains("Tags"),
        "the `name` slice ran past its own property and swallowed a sibling, so the \
         assertions below prove nothing: {name}"
    );
    assert!(
        name.contains("pattern: ^[a-z0-9]([a-z0-9-]*[a-z0-9])?$"),
        "`CreateSnapshotRequest.name`'s grammar moved; `is_legal_snapshot_name` in the \
         node agent is that pattern written out longhand and must move with it. Found: {name}"
    );
    assert!(
        name.contains("maxLength: 63"),
        "`CreateSnapshotRequest.name`'s length limit moved; the node agent mirrors 63. \
         Found: {name}"
    );

    let post = operation(&doc, "/instances/{id}/snapshots", "post");
    let responses = block_under(&post, 6, "responses");
    let conflict = block_under(&responses, 8, "409");
    // Same bound, same reason: the 409 must be *this* operation's.
    assert!(
        !conflict.contains("Not implemented"),
        "the 409 slice ran past its own block and swallowed a sibling response, so the \
         assertion below proves nothing: {conflict}"
    );
    assert!(
        conflict.to_lowercase().contains("duplicate snapshot name"),
        "`POST /instances/{{id}}/snapshots` no longer documents a 409 for a duplicate name; \
         the hypeman runtime maps that status onto SNAPSHOT_NAME_CONFLICT. Found: {conflict}"
    );
}

/// The instance-name budget `sandbox_name` spends, which the node mirrors the way
/// it mirrors the snapshot-name rules above.
///
/// `barista-<node-ulid>-<instance-ulid>` is 61 of these 63 characters (review
/// finding 1 removed a truncation that was buying headroom by giving back
/// cross-node collisions). Two characters of slack is thin enough that a limit
/// moving downstream should be a red gate here rather than a `400` per create on
/// somebody's substrate — and the *grammar* is what `sanitize` is written against,
/// so it is pinned in the same breath.
#[test]
fn the_instance_name_budget_sandbox_name_spends_is_still_the_contracts() {
    let doc = contract();

    let schema = block_under(&doc, 4, "CreateInstanceRequest");
    let properties = block_under(&schema, 6, "properties");
    let name = block_under(&properties, 8, "name");
    // Bounded, for the reason the snapshot-name test records: `image` is `name`'s
    // next sibling, and a slice that ran into it would prove nothing.
    assert!(
        !name.contains("OCI image reference"),
        "the `name` slice ran past its own property and swallowed a sibling, so the \
         assertions below prove nothing: {name}"
    );
    assert!(
        name.contains("maxLength: 63"),
        "`CreateInstanceRequest.name`'s length limit moved; `sandbox_name` spends 61 of \
         it on two whole ULIDs. Found: {name}"
    );
    assert!(
        name.contains("pattern: ^[a-z0-9]([a-z0-9-]*[a-z0-9])?$"),
        "`CreateInstanceRequest.name`'s grammar moved; `HypemanRuntime::sanitize` forces \
         names into that pattern. Found: {name}"
    );
}

/// Absence of a version field is *why* `runtime_bundle_ref` cannot include the
/// substrate's kernel or hypervisor version (design decision 5). If upstream ever
/// exposes one, this test fails and that decision can be improved — a rare case of
/// a test that wants to break.
#[test]
fn substrate_still_exposes_no_version_to_key_on() {
    let doc = contract();
    let health = schema_properties(&doc, "Health");
    assert_eq!(
        health,
        vec!["status".to_string()],
        "`/health` now returns more than `status` — if it exposes a server version, \
         revisit design decision 5: `runtime_bundle_ref` could then include it"
    );
    let instance = schema_properties(&doc, "Instance");
    for absent in ["kernel_version", "hypervisor_version", "resolved_image"] {
        assert!(
            !instance.contains(&absent.to_string()),
            "`Instance.{absent}` now exists — design decision 5 assumed it did not, \
             and `runtime_bundle_ref`/`template_hash` could now use it"
        );
    }
}
