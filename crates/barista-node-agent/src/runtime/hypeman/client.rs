//! Typed client for the operations Barista calls on `hypeman-api`.
//!
//! Hand-written rather than generated: `progenitor` rejects OpenAPI 3.1 and the
//! vendored contract is 3.1.0, Barista calls ~12 of its 58 operations, and `exec` —
//! the surface Barista most depends on — is a WebSocket the document does not
//! describe at all (design decision 2). `vendor/hypeman/openapi.yaml` is pinned,
//! and `tests/hypeman_contract_drift.rs` fails if anything used here moves.
//!
//! Only the fields Barista reads are modelled; serde ignores the rest, so an upstream
//! addition is not a breaking change here — the drift test covers removals.

use serde::{Deserialize, Serialize};

/// Default address of a local `hypeman-api`.
pub const DEFAULT_BASE_URL: &str = "http://127.0.0.1:4973";

/// Bound on establishing a TCP connection to the daemon. It is local, so a
/// connect that takes seconds already means it is not answering.
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Bound on any single HTTP call, connect through body.
///
/// `reqwest::Client` has **no** default timeout, and every runtime verb awaits
/// these calls from inside an executing operation — so a wedged daemon left the
/// op `RUNNING` forever, and the in-flight conflict check then blocked the
/// instance until the next restart. The same starvation shape nap-007 bounded in
/// the reconciler and the guest channel applies here.
///
/// Generous on purpose: the slowest legitimate calls are `from-archive` (an
/// upload plus a host-side `mkfs`) and a stop waiting out the substrate's own
/// grace window. Two minutes is far beyond either while still being an answer.
const HTTP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// The API version this client is written against (`info.version` in the
/// vendored document). Asserted by the drift test.
pub const PINNED_API_VERSION: &str = "0.3.0";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The daemon could not be reached at all. Distinguished from an API error
    /// because it is the difference between "substrate down" and "request wrong"
    /// — and the former must never be read as "the instance is gone".
    #[error("hypeman unreachable at {base_url}: {source}")]
    Unreachable {
        base_url: String,
        #[source]
        source: reqwest::Error,
    },
    #[error("hypeman {status} on {method} {path}: {body}")]
    Api {
        status: u16,
        method: &'static str,
        path: String,
        body: String,
    },
    #[error("decoding hypeman response for {path}: {source}")]
    Decode {
        path: String,
        #[source]
        source: reqwest::Error,
    },
}

impl Error {
    /// Whether this represents the substrate being unreachable, as opposed to it
    /// answering with a refusal.
    pub fn is_unreachable(&self) -> bool {
        matches!(self, Error::Unreachable { .. })
    }

    /// The API's own error code, when it gave one (e.g. `instance_in_standby`).
    pub fn api_code(&self) -> Option<String> {
        let Error::Api { body, .. } = self else {
            return None;
        };
        serde_json::from_str::<ApiError>(body)
            .ok()
            .map(|e| e.code)
            .filter(|c| !c.is_empty())
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// Percent-encode a **path segment**, so a value can never introduce structure.
///
/// Defence in depth behind the ULID check at the API boundary: an id reaching
/// here with a `/` or `..` in it would otherwise reshape the request, and
/// `/instances/../volumes/x` normalises to `/volumes/x` before the request is
/// sent. Encoding is cheaper than trusting every caller upstream to have
/// validated, and correct even for the ids Barista generates itself.
fn path_segment(s: &str) -> String {
    urlencode(s)
}

/// Percent-encode a query value. Tag keys contain dots and values are ULIDs, so
/// this only needs to be correct, not comprehensive.
fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

#[derive(Debug, Deserialize)]
struct ApiError {
    #[serde(default)]
    code: String,
}

// ---------------------------------------------------------------------------
// Contract types — only what Barista reads or sends
// ---------------------------------------------------------------------------

/// `InstanceState` from the vendored contract.
///
/// Note `Paused` vs `Standby`: `Paused` is a Cloud-Hypervisor-native in-memory
/// pause that keeps the VM resident, while `Standby` is a snapshot to disk.
/// Barista's `PAUSED` — "holds zero sandbox resources" — is `Standby`
/// (design decision 6). Confusing the two would keep every "paused" session
/// resident and destroy the point of the platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum InstanceState {
    Created,
    Initializing,
    Running,
    Paused,
    Shutdown,
    Stopped,
    Standby,
    Unknown,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Instance {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub image: String,
    pub state: InstanceState,
    #[serde(default)]
    pub state_error: Option<String>,
    /// Hypervisor **type** (`vz`, `firecracker`, …). Deliberately not a version:
    /// the API exposes none, which is why `runtime_bundle_ref` cannot include one
    /// (design decision 5).
    #[serde(default)]
    pub hypervisor: Option<String>,
    #[serde(default)]
    pub tags: std::collections::HashMap<String, String>,
    #[serde(default)]
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub has_snapshot: Option<bool>,
    /// How this instance's memory was produced when it was created by a fork
    /// (barista-046): `shared` (copy-on-write via a shared mem-file) or `copied`
    /// (full copy). `None` on an instance not created by a fork, and on a hypeman
    /// too old to report it (kernel/hypeman#419) — which is why it is optional and
    /// the runtime degrades honestly rather than assuming a mode.
    #[serde(default)]
    pub fork_mode: Option<String>,
    /// Network placement. Its `ip` is the guest channel's address — this
    /// substrate has no other way to hand the host a byte stream to a process
    /// inside the VM (design decision 5b).
    #[serde(default)]
    pub network: Option<InstanceNetwork>,
}

/// One substrate snapshot. Only the fields Barista keys on are modelled.
#[derive(Debug, Clone, Deserialize)]
pub struct Snapshot {
    pub id: String,
    /// `Standby` (memory captured) or `Stopped` (disk only). This is what
    /// `Snapshot.kind` reports to the caller, and it is read rather than inferred
    /// from the runtime's capabilities: capability is what a runtime can usually
    /// do, this is what it did.
    pub kind: SnapshotKind,
    #[serde(default)]
    pub source_instance_name: String,
    #[serde(default)]
    pub size_bytes: u64,
}

/// The substrate's snapshot kinds. `Standby` keeps memory; `Stopped` does not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum SnapshotKind {
    Standby,
    Stopped,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Deserialize)]
pub struct InstanceNetwork {
    #[serde(default)]
    pub enabled: Option<bool>,
    /// Documented as always `"default"` when enabled — i.e. **one network per
    /// host**, not one per instance. Read, and carried, because that is exactly
    /// what makes the guest's port reachable by sibling VMs and is the reason the
    /// token is load-bearing rather than belt-and-braces.
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub ip: Option<String>,
}

impl Instance {
    /// The address the guest agent is reachable at, when it has one.
    pub fn ip(&self) -> Option<&str> {
        self.network
            .as_ref()
            .and_then(|n| n.ip.as_deref())
            .filter(|ip| !ip.is_empty())
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct CreateInstanceRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub image: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vcpus: Option<u32>,
    /// Writable overlay size. Absent means the substrate's default, which is
    /// 10 GB per instance — enough to exhaust a modest host after a handful of
    /// sandboxes, and not what a caller asking for 256 MiB expects.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overlay_size: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hypervisor: Option<String>,
    /// Per-instance environment — how the guest token reaches Contract C.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env: Option<std::collections::HashMap<String, String>>,
    /// Node-scoped ownership label, so reconciliation never reaps a peer node's
    /// sandbox (`node-agent-api` "deterministic crash recovery").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<std::collections::HashMap<String, String>>,
    /// Entrypoint override — this is how `barista-guest-agent serve` wraps the
    /// workload without modifying the image.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entrypoint: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cmd: Option<Vec<String>>,
    /// How the guest agent reaches a VM: read-only, content-addressed
    /// (design decision 3).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volumes: Option<Vec<VolumeMount>>,
    /// Egress policy, and nothing else the `network` object can carry
    /// (nap-014). Absent means the substrate's default networking, which is the
    /// only honest rendering of a spec that declared no policy.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network: Option<NetworkConfig>,
}

/// The create request's `network` object, modelled only as far as Barista sends it.
///
/// `enabled`, `bandwidth_download` and `bandwidth_upload` are deliberately
/// absent: Barista's contract has no surface for them, and sending the substrate's
/// own defaults back to it would turn a future default change into a silent
/// override by Barista.
#[derive(Debug, Clone, Serialize)]
pub struct NetworkConfig {
    pub egress: EgressConfig,
}

/// `network.egress` — the substrate's host-mediated outbound path.
///
/// Barista only ever builds this to turn mediation **on**. The contract says an
/// omitted object and `enabled: false` mean the same thing, so the runtime omits
/// the whole object rather than sending a disabled one: the two spellings are
/// equivalent only for as long as upstream's default holds, and one of them does
/// not depend on that.
#[derive(Debug, Clone, Serialize)]
pub struct EgressConfig {
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enforcement: Option<EgressEnforcement>,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct EgressEnforcement {
    pub mode: EgressMode,
}

/// The substrate's enforcement modes, in its own spelling.
///
/// `snake_case` is not cosmetic: these are the two literals the contract's enum
/// declares (`all`, `http_https_only`), and the drift test pins them, because a
/// rename upstream would otherwise surface as a 400 on every mediated create.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EgressMode {
    All,
    HttpHttpsOnly,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Health {
    pub status: String,
}

/// One ingress object — the substrate's embedded reverse proxy, and the
/// mechanism behind the workload endpoint (barista-040). Only the fields
/// Barista reads are modelled.
#[derive(Debug, Clone, Deserialize)]
pub struct Ingress {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub rules: Vec<IngressRule>,
    #[serde(default)]
    pub tags: std::collections::HashMap<String, String>,
}

/// One routing rule: what to match on the host, and which instance port it
/// reaches. `tls`/`redirect_http` are deliberately not sent — their defaults
/// are the substrate's, and the node's listener is plaintext behind the
/// operator's firewall by design (the gateway owns public TLS).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngressRule {
    // serde uses the identifier without the raw prefix, so this serializes as
    // the contract's `match`.
    pub r#match: IngressMatch,
    pub target: IngressTarget,
}

/// The host side of a rule. `hostname` is a literal Host-header match (the
/// contract's other form — `{capture}` patterns — is not used: Barista's
/// callers dial by the advertised host, not per-instance DNS). `port` is the
/// host listener; the contract defaults it to 80 when absent, which is why it
/// is optional on read — Barista always sends one.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngressMatch {
    pub hostname: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
}

/// The instance side of a rule. `instance` takes a name or id; Barista sends
/// the sandbox *name*, because it is the identity that survives the cold-boot
/// delete-and-recreate path while an id dies with its sandbox.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngressTarget {
    pub instance: String,
    pub port: u16,
}

#[derive(Debug, Clone, Serialize)]
pub struct CreateIngressRequest {
    /// Same grammar and 63-char budget as instance names; Barista reuses the
    /// sandbox name, which is proven to fit.
    pub name: String,
    pub rules: Vec<IngressRule>,
    /// Node/instance ownership, the tagging rule every substrate object
    /// Barista creates follows.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<std::collections::HashMap<String, String>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Volume {
    pub id: String,
    #[serde(default)]
    pub name: String,
    /// Ownership claim, read by the credential sweep. Defaulted rather than
    /// optional: a volume created before this field existed has no claim, and
    /// "no claim" is a verdict the sweep acts on (it reports, never deletes).
    #[serde(default)]
    pub tags: std::collections::HashMap<String, String>,
}

/// One volume attached to a sandbox. The agent arrives this way, since a VM has no
/// bind mount (design decision 3).
#[derive(Debug, Clone, Serialize)]
pub struct VolumeMount {
    pub volume_id: String,
    pub mount_path: String,
    pub readonly: bool,
}

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct HypemanClient {
    http: reqwest::Client,
    base_url: String,
    token: Option<String>,
}

impl HypemanClient {
    pub fn new(base_url: impl Into<String>, token: Option<String>) -> Self {
        Self {
            http: reqwest::Client::builder()
                .connect_timeout(CONNECT_TIMEOUT)
                .timeout(HTTP_TIMEOUT)
                .build()
                // Static configuration, no TLS backend to initialise: this cannot
                // fail for a reason a caller could handle.
                .expect("building the hypeman HTTP client"),
            base_url: base_url.into().trim_end_matches('/').to_string(),
            token,
        }
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let builder = self
            .http
            .request(method, format!("{}{path}", self.base_url));
        match &self.token {
            // The token is a secret: it goes on the wire and nowhere else — never
            // into an error message or a log line.
            Some(token) => builder.bearer_auth(token),
            None => builder,
        }
    }

    async fn send<T: for<'de> Deserialize<'de>>(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&impl Serialize>,
    ) -> Result<T> {
        let name: &'static str = match method {
            reqwest::Method::GET => "GET",
            reqwest::Method::POST => "POST",
            reqwest::Method::DELETE => "DELETE",
            _ => "OTHER",
        };
        let mut req = self.request(method, path);
        if let Some(body) = body {
            req = req.json(body);
        }
        let response = req.send().await.map_err(|source| Error::Unreachable {
            base_url: self.base_url.clone(),
            source,
        })?;
        let status = response.status();
        if !status.is_success() {
            return Err(Error::Api {
                status: status.as_u16(),
                method: name,
                path: path.to_string(),
                body: response.text().await.unwrap_or_default(),
            });
        }
        response.json().await.map_err(|source| Error::Decode {
            path: path.to_string(),
            source,
        })
    }

    /// Like [`send`] but for endpoints that answer with no useful body.
    async fn send_unit(&self, method: reqwest::Method, path: &str) -> Result<()> {
        self.send_unit_with(method, path, None).await
    }

    /// [`send_unit`] for an endpoint that *requires* a request body even when
    /// every property in it is optional.
    ///
    /// `POST /instances/{id}/start` is one: it declares `requestBody: required`
    /// with only override fields inside, so an empty object is both valid and the
    /// meaning we want ("keep the previous entrypoint and cmd"). Sending nothing
    /// at all fails the schema check with
    /// `value is required but missing` — a 400 that reads like a missing *field*
    /// rather than a missing body, which is what made it slow to place.
    async fn send_unit_with(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<()> {
        let name: &'static str = if method == reqwest::Method::DELETE {
            "DELETE"
        } else {
            "POST"
        };
        let mut request = self.request(method, path);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.map_err(|source| Error::Unreachable {
            base_url: self.base_url.clone(),
            source,
        })?;
        let status = response.status();
        if status.is_success() {
            return Ok(());
        }
        Err(Error::Api {
            status: status.as_u16(),
            method: name,
            path: path.to_string(),
            body: response.text().await.unwrap_or_default(),
        })
    }

    pub async fn health(&self) -> Result<Health> {
        self.send(reqwest::Method::GET, "/health", None::<&()>)
            .await
    }

    pub async fn create_instance(&self, req: &CreateInstanceRequest) -> Result<Instance> {
        self.send(reqwest::Method::POST, "/instances", Some(req))
            .await
    }

    pub async fn get_instance(&self, id: &str) -> Result<Instance> {
        self.send(
            reqwest::Method::GET,
            &format!("/instances/{}", path_segment(id)),
            None::<&()>,
        )
        .await
    }

    /// List instances, optionally filtered to one tag — the node-scoped sweep.
    pub async fn list_instances(&self, tag: Option<(&str, &str)>) -> Result<Vec<Instance>> {
        // deepObject style: `?tags[key]=value`. A `tags=k%3Dv` form is silently
        // ignored rather than rejected, so the filter appeared to work while
        // returning every instance on the host — including other nodes'.
        let path = match tag {
            Some((k, v)) => format!("/instances?tags[{}]={}", urlencode(k), urlencode(v)),
            None => "/instances".to_string(),
        };
        self.send(reqwest::Method::GET, &path, None::<&()>).await
    }

    /// Restart a stopped instance, keeping the entrypoint and cmd it was created
    /// with — hence the empty object rather than no body at all.
    pub async fn start_instance(&self, id: &str) -> Result<()> {
        self.send_unit_with(
            reqwest::Method::POST,
            &format!("/instances/{}/start", path_segment(id)),
            Some(serde_json::json!({})),
        )
        .await
    }

    pub async fn stop_instance(&self, id: &str) -> Result<()> {
        self.send_unit(
            reqwest::Method::POST,
            &format!("/instances/{}/stop", path_segment(id)),
        )
        .await
    }

    /// Barista's `Pause`: snapshot to disk and release the VM.
    pub async fn standby_instance(&self, id: &str) -> Result<()> {
        self.send_unit(
            reqwest::Method::POST,
            &format!("/instances/{}/standby", path_segment(id)),
        )
        .await
    }

    /// Barista's `Resume`.
    pub async fn restore_instance(&self, id: &str) -> Result<()> {
        self.send_unit(
            reqwest::Method::POST,
            &format!("/instances/{}/restore", path_segment(id)),
        )
        .await
    }

    pub async fn get_volume(&self, id_or_name: &str) -> Result<Volume> {
        self.send(
            reqwest::Method::GET,
            &format!("/volumes/{}", path_segment(id_or_name)),
            None::<&()>,
        )
        .await
    }

    /// List volumes, optionally filtered to one tag — the node-scoped credential
    /// sweep, and the same deepObject spelling `list_instances` needs.
    ///
    /// The unfiltered form is not redundant with the filtered one: the sweep has
    /// to see volumes carrying *no* claim in order to report them, and a tag
    /// filter is exactly what hides those.
    pub async fn list_volumes(&self, tag: Option<(&str, &str)>) -> Result<Vec<Volume>> {
        let path = match tag {
            Some((k, v)) => format!("/volumes?tags[{}]={}", urlencode(k), urlencode(v)),
            None => "/volumes".to_string(),
        };
        self.send(reqwest::Method::GET, &path, None::<&()>).await
    }

    /// Create a volume pre-populated from a `tar.gz`. The archive is the only way
    /// to get a host-side file into a VM's filesystem.
    pub async fn create_volume_from_archive(
        &self,
        id: &str,
        name: &str,
        size_gb: u32,
        tags: &[(&str, &str)],
        archive: Vec<u8>,
    ) -> Result<Volume> {
        // The explicit `id` is what makes this idempotent. Names are NOT unique —
        // creating twice by name yields two volumes and then every lookup by name
        // fails with `ambiguous` (measured the hard way).
        let mut path = format!("/volumes/from-archive?name={name}&size_gb={size_gb}&id={id}");
        // Same deepObject spelling as the instance filter, and for the same
        // reason: a `tags=k%3Dv` form is accepted and ignored, which would leave
        // every credential unclaimed while looking like it worked.
        for (k, v) in tags {
            path.push_str(&format!("&tags[{}]={}", urlencode(k), urlencode(v)));
        }
        let response = self
            .request(reqwest::Method::POST, &path)
            .header(reqwest::header::CONTENT_TYPE, "application/gzip")
            .body(archive)
            .send()
            .await
            .map_err(|source| Error::Unreachable {
                base_url: self.base_url.clone(),
                source,
            })?;
        let status = response.status();
        if !status.is_success() {
            return Err(Error::Api {
                status: status.as_u16(),
                method: "POST",
                path,
                body: response.text().await.unwrap_or_default(),
            });
        }
        response.json().await.map_err(|source| Error::Decode {
            path: "/volumes/from-archive".to_string(),
            source,
        })
    }

    /// Idempotent by contract: an already-absent instance is success, because
    /// journaled compensation replays destroy.
    pub async fn delete_instance(&self, id: &str) -> Result<()> {
        match self
            .send_unit(
                reqwest::Method::DELETE,
                &format!("/instances/{}", path_segment(id)),
            )
            .await
        {
            Ok(()) => Ok(()),
            Err(Error::Api { status: 404, .. }) => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Restore in place from a **specific** snapshot, as opposed to the
    /// instance's own latest that `restore_instance` uses.
    ///
    /// The empty body is load-bearing: the contract declares
    /// `requestBody: required` with only optional properties inside
    /// (`target_state`, `target_hypervisor`), exactly the `start` shape — and the
    /// drift test's body table is what caught this one *before* a live 400
    /// rather than after (nap-010 task 1.2; the nap-005 5.1 lesson paying out).
    pub async fn restore_instance_snapshot(&self, id: &str, snapshot_id: &str) -> Result<()> {
        self.send_unit_with(
            reqwest::Method::POST,
            &format!(
                "/instances/{}/snapshots/{}/restore",
                path_segment(id),
                path_segment(snapshot_id)
            ),
            Some(serde_json::json!({})),
        )
        .await
    }

    /// Create an **explicit, retained** snapshot of an instance — the thing
    /// `standby` does not do (its image is instance-internal and unlisted).
    ///
    /// `kind: Standby` asks for memory + disk, which is the only kind Barista ever
    /// creates explicitly: a disk-only artifact is what `Stop` already leaves.
    ///
    /// `name` is optional and, per the contract, unique per source instance — a
    /// duplicate is a 409 the caller must be told about rather than a second
    /// artifact under a shared label (nap-015). It is **omitted** when absent
    /// rather than sent empty: the schema constrains it to
    /// `^[a-z0-9]([a-z0-9-]*[a-z0-9])?$`, which an empty string does not match,
    /// so sending one would fail the schema check instead of meaning "unnamed".
    pub async fn create_instance_snapshot(
        &self,
        id_or_name: &str,
        name: Option<&str>,
    ) -> Result<Snapshot> {
        let mut body = serde_json::json!({ "kind": "Standby" });
        if let Some(name) = name.filter(|n| !n.is_empty()) {
            body["name"] = serde_json::Value::String(name.to_string());
        }
        self.send(
            reqwest::Method::POST,
            &format!("/instances/{}/snapshots", path_segment(id_or_name)),
            Some(&body),
        )
        .await
    }

    /// Fork a retained snapshot into a new instance (barista-046 §3.4 →
    /// kernel/hypeman#419). `POST /snapshots/{id}/fork` clones the snapshot's
    /// exact state into a fresh instance named `name`, in `target_state`
    /// ("Running" to bring the branch up live). The returned instance's
    /// `fork_mode` is the measured mode (`shared` = copy-on-write, `copied` =
    /// full copy); the source snapshot is untouched.
    pub async fn fork_snapshot(
        &self,
        snapshot_id: &str,
        name: &str,
        target_state: &str,
    ) -> Result<Instance> {
        let body = serde_json::json!({ "name": name, "target_state": target_state });
        self.send(
            reqwest::Method::POST,
            &format!("/snapshots/{}/fork", path_segment(snapshot_id)),
            Some(&body),
        )
        .await
    }

    /// Snapshots the substrate holds for one instance.
    ///
    /// The collection lives at `GET /snapshots` filtered by source, **not** at
    /// `GET /instances/{id}/snapshots` — that path exists but is POST-only
    /// (create). Reading it returned a bare `405` with no body, which is why this
    /// took a live run to find rather than a compile or a schema check.
    ///
    /// Takes an id **or a name**, because every other instance operation does and
    /// callers hold the name. The query filter is by canonical id only, so the
    /// name is resolved here rather than at each call site — one extra `GET`, and
    /// the alternative is a filter that silently matches nothing when handed a
    /// name, which is the worst possible failure for a "what snapshots exist"
    /// question.
    pub async fn list_instance_snapshots(&self, id_or_name: &str) -> Result<Vec<Snapshot>> {
        let instance_id = self.get_instance(id_or_name).await?.id;
        self.send(
            reqwest::Method::GET,
            &format!(
                "/snapshots?source_instance_id={}",
                path_segment(&instance_id)
            ),
            None::<&()>,
        )
        .await
    }

    pub async fn get_snapshot(&self, snapshot_id: &str) -> Result<Snapshot> {
        self.send(
            reqwest::Method::GET,
            &format!("/snapshots/{}", path_segment(snapshot_id)),
            None::<&()>,
        )
        .await
    }

    /// Idempotent by contract: `destroy` replays through journaled compensation,
    /// and a snapshot already gone is the state the caller wanted.
    pub async fn delete_snapshot(&self, snapshot_id: &str) -> Result<()> {
        match self
            .send_unit(
                reqwest::Method::DELETE,
                &format!("/snapshots/{}", path_segment(snapshot_id)),
            )
            .await
        {
            Ok(()) => Ok(()),
            Err(Error::Api { status: 404, .. }) => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Delete a volume. The 404 is deliberately **not** swallowed here, unlike
    /// `delete_instance`: the caller that matters is token-volume cleanup, and it
    /// wants to distinguish "already gone" from "gone wrong" so a credential left
    /// behind is never mistaken for one removed.
    pub async fn delete_volume(&self, id: &str) -> Result<()> {
        self.send_unit(
            reqwest::Method::DELETE,
            &format!("/volumes/{}", path_segment(id)),
        )
        .await
    }

    /// Publish an ingress (barista-040). A `409` is an answer the caller
    /// branches on — the name already exists (a replay of us won) or the
    /// hostname+port is taken (a lost allocation race) — so it is not mapped
    /// away here.
    pub async fn create_ingress(&self, req: &CreateIngressRequest) -> Result<Ingress> {
        self.send(reqwest::Method::POST, "/ingresses", Some(req))
            .await
    }

    /// `{id}` accepts an id or a name, like the instance operations; ingress
    /// names are unique by contract (creation answers a duplicate with 409),
    /// so a by-name read is unambiguous in the way a volume's is not.
    pub async fn get_ingress(&self, id_or_name: &str) -> Result<Ingress> {
        self.send(
            reqwest::Method::GET,
            &format!("/ingresses/{}", path_segment(id_or_name)),
            None::<&()>,
        )
        .await
    }

    /// List ingresses, optionally filtered to one tag — the same deepObject
    /// spelling every other listing needs. The unfiltered form is the port
    /// allocator's view: a listener port is host-global, so ports held by
    /// objects Barista did not create still count as taken.
    pub async fn list_ingresses(&self, tag: Option<(&str, &str)>) -> Result<Vec<Ingress>> {
        let path = match tag {
            Some((k, v)) => format!("/ingresses?tags[{}]={}", urlencode(k), urlencode(v)),
            None => "/ingresses".to_string(),
        };
        self.send(reqwest::Method::GET, &path, None::<&()>).await
    }

    /// Idempotent by contract, like `delete_instance`: `destroy` replays
    /// through journaled compensation, and an ingress already gone is the
    /// state the caller wanted.
    pub async fn delete_ingress(&self, id_or_name: &str) -> Result<()> {
        match self
            .send_unit(
                reqwest::Method::DELETE,
                &format!("/ingresses/{}", path_segment(id_or_name)),
            )
            .await
        {
            Ok(()) => Ok(()),
            Err(Error::Api { status: 404, .. }) => Ok(()),
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unreachable_is_distinguishable_from_refusal() {
        let api = Error::Api {
            status: 409,
            method: "POST",
            path: "/instances/x/restore".into(),
            body: r#"{"code":"instance_in_standby","message":"restore it first"}"#.into(),
        };
        assert!(!api.is_unreachable());
        assert_eq!(api.api_code().as_deref(), Some("instance_in_standby"));
    }

    #[test]
    fn base_url_trailing_slash_does_not_double_up() {
        let c = HypemanClient::new("http://127.0.0.1:4973/", None);
        assert_eq!(c.base_url, "http://127.0.0.1:4973");
    }

    #[test]
    fn barista_paused_maps_to_standby_not_paused() {
        // Guards design decision 6: hypeman's `Paused` keeps the VM resident.
        let standby: InstanceState = serde_json::from_str("\"Standby\"").unwrap();
        let paused: InstanceState = serde_json::from_str("\"Paused\"").unwrap();
        assert_ne!(standby, paused);
        assert_eq!(standby, InstanceState::Standby);
    }

    /// A forked instance reports its measured fork mode (barista-046 §3.4); an
    /// instance not created by a fork — or one from a hypeman too old to report it
    /// — leaves it `None`, which the runtime treats conservatively as full-copy.
    #[test]
    fn fork_mode_is_parsed_and_optional() {
        let forked: Instance =
            serde_json::from_str(r#"{"id":"i1","state":"Running","fork_mode":"shared"}"#).unwrap();
        assert_eq!(forked.fork_mode.as_deref(), Some("shared"));

        let plain: Instance = serde_json::from_str(r#"{"id":"i2","state":"Running"}"#).unwrap();
        assert_eq!(plain.fork_mode, None, "absent fork_mode must not error");
    }

    /// The two mode literals go on the wire exactly as the contract spells them.
    ///
    /// Worth a test because Rust's spelling and the contract's differ, and the
    /// mapping between them is a serde attribute — the kind of thing a
    /// `rename_all` change elsewhere in the file would break without any type
    /// error. `HTTPHTTPSOnly`, `httpHttpsOnly` and `http-https-only` are all
    /// plausible renderings and all of them are a 400.
    #[test]
    fn egress_modes_serialize_in_the_contracts_spelling() {
        assert_eq!(serde_json::to_string(&EgressMode::All).unwrap(), "\"all\"");
        assert_eq!(
            serde_json::to_string(&EgressMode::HttpHttpsOnly).unwrap(),
            "\"http_https_only\""
        );
    }

    /// The shape of a mediated create, nested exactly as the contract nests it:
    /// `network.egress.enforcement.mode`. A flattened or misnamed level here is
    /// ignored rather than rejected — the object is optional — so the sandbox
    /// would boot with open egress and nothing would say so.
    #[test]
    fn a_mediated_create_nests_the_egress_object_where_the_contract_wants_it() {
        let req = CreateInstanceRequest {
            image: "busybox:latest".into(),
            network: Some(NetworkConfig {
                egress: EgressConfig {
                    enabled: true,
                    enforcement: Some(EgressEnforcement {
                        mode: EgressMode::HttpHttpsOnly,
                    }),
                },
            }),
            ..Default::default()
        };
        let json: serde_json::Value = serde_json::from_str(&serde_json::to_string(&req).unwrap())
            .expect("the request serializes");
        assert_eq!(json["network"]["egress"]["enabled"], true);
        assert_eq!(
            json["network"]["egress"]["enforcement"]["mode"],
            "http_https_only"
        );
    }

    /// The ingress create goes on the wire in the contract's shape: `match`
    /// (the keyword survives serde's raw-identifier stripping), `target`, and
    /// the ports where the contract wants them. A misnamed level here would be
    /// a 400 on every publish — or worse, an accepted rule matching nothing.
    #[test]
    fn an_ingress_create_nests_match_and_target_where_the_contract_wants_them() {
        let req = CreateIngressRequest {
            name: "barista-node-inst".into(),
            rules: vec![IngressRule {
                r#match: IngressMatch {
                    hostname: "node.example".into(),
                    port: Some(30000),
                },
                target: IngressTarget {
                    instance: "barista-node-inst".into(),
                    port: 30000,
                },
            }],
            tags: None,
        };
        let json: serde_json::Value = serde_json::from_str(&serde_json::to_string(&req).unwrap())
            .expect("the request serializes");
        assert_eq!(json["rules"][0]["match"]["hostname"], "node.example");
        assert_eq!(json["rules"][0]["match"]["port"], 30000);
        assert_eq!(json["rules"][0]["target"]["instance"], "barista-node-inst");
        assert_eq!(json["rules"][0]["target"]["port"], 30000);
        assert!(
            !serde_json::to_string(&req).unwrap().contains("null"),
            "unset fields must be omitted"
        );
    }

    /// A rule read back without a listener port means the contract's default,
    /// 80 — optional on read because the substrate may omit what it defaulted,
    /// even though Barista always sends one.
    #[test]
    fn an_ingress_rule_reads_back_with_or_without_its_port() {
        let ing: Ingress = serde_json::from_str(
            r#"{"id":"x","name":"n","rules":[{"match":{"hostname":"h"},"target":{"instance":"i","port":8080}}]}"#,
        )
        .unwrap();
        assert_eq!(ing.rules[0].r#match.port, None);
        assert_eq!(ing.rules[0].target.port, 8080);
    }

    #[test]
    fn create_request_omits_unset_fields() {
        let req = CreateInstanceRequest {
            image: "busybox:latest".into(),
            entrypoint: Some(vec!["/barista/barista-guest-agent".into(), "serve".into()]),
            ..Default::default()
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"image\":\"busybox:latest\""));
        assert!(json.contains("barista-guest-agent"));
        assert!(
            !json.contains("null"),
            "unset fields must be omitted: {json}"
        );
    }
}
