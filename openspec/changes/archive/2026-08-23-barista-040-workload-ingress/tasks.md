# Tasks: barista-040-workload-ingress

## 1. Substrate client surface

- [x] 1.1 Model the ingress contract types in `runtime/hypeman/client.rs`
      (`Ingress`, `IngressRule`, `IngressMatch`, `IngressTarget`,
      `CreateIngressRequest`) — only the fields Barista reads or sends — and
      add `create_ingress`, `get_ingress`, `list_ingresses` (tag-filterable,
      deepObject), `delete_ingress` (404 is success: destroy replays).
- [x] 1.2 Pin the surface in `tests/hypeman_contract_drift.rs`: the four
      operations, the required request body, the schema properties the
      client depends on, and the 409 conflict answer the allocator branches
      on.

## 2. Configuration

- [x] 2.1 `IngressConfig { advertise_host, ports }` in a new
      `runtime/hypeman/ingress.rs`, with boundary validation: bare host only
      (no scheme, port or path), non-empty range excluding port 0.
- [x] 2.2 `--ingress-advertise` (`BARISTA_INGRESS_ADVERTISE`) and
      `--ingress-ports` (`BARISTA_INGRESS_PORTS`, default `30000-30999`) in
      `main.rs`, threaded into `HypemanRuntime::connect`; absent advertise ⇒
      no ingress anywhere, the fleet's laptop-mode pattern; the flag with a
      non-hypeman runtime is refused rather than silently ignored.

## 3. Plan / publish / report

- [x] 3.1 `ingress::planned_port`: GET by sandbox name and reuse its listener
      port (stickiness); otherwise pick the lowest free port from the
      unfiltered listing (reserving nothing — the substrate arbitrates at
      publish); exhausted range fails naming the knob. `ingress::publish`:
      converge the object to `{listener → target}` once the sandbox exists;
      correct a drifted target under the same listener; propagate a 409 so
      the failed create retries on a fresh plan. (Ordering forced by the
      substrate: `POST /ingresses` refuses a target it does not know —
      measured live, design decision 4, upstream findings §12.)
- [x] 3.2 Wire `create_fresh`: plan before the sandbox, inject `PORT` into
      the process env fed to `ENV_PROCESS` (absent-only; honour a
      spec-supplied `PORT` as the guest target; refuse an unparseable one at
      create), publish the moment the sandbox exists, roll the sandbox back
      on a refused publish.
- [x] 3.3 `workload_address`: with ingress configured, report
      `<advertise>:<listener>` read live from the ingress object; degrade to
      absence on any failure; **stop reporting the guest IP** in every case.
- [x] 3.4 `destroy` deletes the ingress (before the sandbox), idempotently
      and unconditionally, so an object created under an earlier
      configuration cannot outlive its instance; `remove_orphan` inherits it
      via `destroy`.

## 4. Tests and docs

- [x] 4.1 Unit: port picking (lowest free, skips used, exhaustion), `PORT`
      precedence and refusal, advertise/range validation, multi-rule refusal,
      create-request env assertions with and without an allocated port,
      absence-not-guest-IP and degrade-to-absence on `workload_address`,
      ingress request/response serde shapes.
- [x] 4.2 Hypeman-gated integration (`tests/session_ingress.rs`), **run green
      against the live substrate on this host (macOS/vz, 2026-08-13)**:
      create on a publishing node ⇒ address is `<advertise>:<port>` in
      range; the listener answers a request carrying that Host (502 — routed,
      guest hop broken on macOS; a wedged local Caddy soft-skips with a note,
      findings §12); the ingress object targets the sandbox name; a second
      instance gets its own port; pause ⇒ address absent, object survives;
      resume ⇒ byte-for-byte the same address; destroy ⇒ ingress 404; a
      spec-supplied `PORT` becomes the target under a range listener.
- [x] 4.3 Update `tests/instance_endpoint.rs` to the modified requirement —
      an unpublishing node reports no address for a RUNNING instance (run
      green against the live substrate); the fake negative unchanged;
      `service.rs` unit tests stay green (enrichment gating unchanged).
- [x] 4.4 Docs: `docs/concepts/networking-and-egress.md` ("Published
      workloads" — endpoint, `$PORT`, stickiness, what is deliberately not
      built) and `docs/api/index.md` field reference; upstream findings §12
      records the measured substrate behaviours this design rests on.
- [x] 4.5 `openspec validate --all --strict` green (23/23); `cargo fmt`,
      `cargo clippy -D warnings`, `cargo test --workspace` green (47
      binaries); `task docs` green. **Verified live on macOS/vz:** the whole
      ingress lifecycle of 4.2/4.3 including the listener dial. **Not
      exercisable on macOS:** dialling *through* the listener to the guest
      (hypeman #358) and reading `$PORT` inside the guest — the end-to-end
      serve is verified on the Linux node; `PORT`-in-guest is compositional
      (unit-pinned injection + the agent's env application, t6). `buf lint`
      / `gen-check` / `cargo-deny` not run (tools absent locally; no proto
      or dependency change).
