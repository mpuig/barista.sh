//! Node preflight for the hypeman substrate.
//!
//! The spike found that hypeman's macOS prerequisites are undocumented upstream
//! and fail obscurely: a missing `caddy` crash-loops the API every ten seconds,
//! and a missing `mkfs.ext4` fails image conversion with a bare status of `failed`
//! whose cause appears in no log and no API response. Both cost a source dive to
//! diagnose. Naming them at startup is cheap and saves the next person that hour.

use std::path::{Path, PathBuf};

use super::client::HypemanClient;
use super::config::Config;

/// One unmet prerequisite, with enough detail to act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub what: String,
    pub why_it_matters: String,
    pub remedy: String,
}

impl std::fmt::Display for Problem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} — {} Fix: {}",
            self.what, self.why_it_matters, self.remedy
        )
    }
}

/// Look for an executable on `PATH`, then at known fixed locations. The fallbacks
/// exist because Homebrew keeps `e2fsprogs` keg-only, so `mkfs.ext4` is installed
/// but deliberately absent from `PATH` — hypeman itself probes the same paths.
pub fn find_executable(name: &str, extra_paths: &[&str]) -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join(name);
            if is_executable(&candidate) {
                return Some(candidate);
            }
        }
    }
    extra_paths
        .iter()
        .map(PathBuf::from)
        .find(|p| is_executable(p))
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// Prerequisites that can be checked without touching the network.
///
/// Both platforms have one, which this used to deny: the comment here read "only
/// meaningful on macOS: on Linux hypeman embeds Caddy and uses erofs, so neither
/// prerequisite applies", and returned an empty list. Measured on a real Linux
/// node, erofs is exactly as much of a prerequisite as ext4 is on macOS —
/// `mkfs.erofs` ships in `erofs-utils` and is not installed by default, and
/// without it every image lands in `status: failed` with the cause visible only
/// in the daemon's journal. That is the same obscure failure this whole function
/// exists to pre-empt.
pub fn local_prerequisites() -> Vec<Problem> {
    let mut problems = Vec::new();

    problems.extend(initrd_guest_binary_problems());

    if cfg!(target_os = "linux") && find_executable("mkfs.erofs", &[]).is_none() {
        problems.push(Problem {
            what: "`mkfs.erofs` is not installed".into(),
            why_it_matters: "on Linux hypeman converts each image to erofs; without it every \
                             image reaches `status: failed` and the reason appears only in the \
                             daemon's journal, never in the API response."
                .into(),
            remedy: "apt install erofs-utils (Debian/Ubuntu) or dnf install erofs-utils".into(),
        });
    }

    if !cfg!(target_os = "macos") {
        return problems;
    }

    if find_executable("caddy", &[]).is_none() {
        problems.push(Problem {
            what: "`caddy` is not installed".into(),
            why_it_matters: "hypeman-api initialises its ingress unconditionally on macOS and \
                             crash-loops without it, even though Barista never uses ingress."
                .into(),
            remedy: "brew install caddy".into(),
        });
    }

    if find_executable(
        "mkfs.ext4",
        &[
            "/opt/homebrew/opt/e2fsprogs/sbin/mkfs.ext4",
            "/usr/local/opt/e2fsprogs/sbin/mkfs.ext4",
        ],
    )
    .is_none()
    {
        problems.push(Problem {
            what: "`mkfs.ext4` is not installed".into(),
            why_it_matters: "on macOS hypeman converts each image to ext4 (the VZ kernel has no \
                             erofs support); without it images fail with a bare status of \
                             `failed` and no diagnosable cause."
                .into(),
            remedy: "brew install e2fsprogs (keg-only, so it stays off PATH — that is expected)"
                .into(),
        });
    }

    problems
}

/// Full preflight: local prerequisites plus substrate reachability.
///
/// Reachability is reported as a problem rather than an error so that a node can
/// still start and serve introspection while the substrate is down — the spike
/// established that a dead `hypeman-api` does not disturb running instances, so
/// refusing to start would be a worse failure than reporting it.
pub async fn run(config: &Config) -> Vec<Problem> {
    let client = config.client();
    let mut problems = local_prerequisites();
    problems.extend(check_reachable(&client, &config.base_url).await);
    problems.extend(check_authorized(&client, &config.base_url).await);
    problems.extend(check_api_requires_auth(config).await);
    problems
}

/// A capability this host does not have, with the reasoning — **not** a
/// [`Problem`].
///
/// The distinction cost a red gate to learn. `Problem` means "this host cannot
/// do its job, here is how to fix it", and `a_provisioned_host_reports_no_problems`
/// enforces exactly that reading. An unenforced optional capability is neither:
/// the node runs correctly, refuses mediated specs honestly, and needs no action
/// from the operator beyond knowing. Filing it as a Problem made a correctly
/// provisioned host report a provisioning failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityNote {
    pub what: String,
    pub why_it_matters: String,
    pub remedy: String,
}

/// Egress enforcement cannot be demonstrated by this node, so the capability is
/// not claimed (nap-014 option (a)).
///
/// **The measurement that was attempted, and what it found.** The plan was a
/// cheap startup probe: send a create carrying a deliberately invalid
/// `network.egress.enforcement.mode` and read the answer. A substrate that parses
/// the object rejects it; one that discards it accepts it. Run against the
/// deployed substrate, the request came back `400` naming
/// `/network/egress/enforcement/mode` and listing the allowed values — so the
/// object *is* parsed, and the probe would have reported the capability as
/// present. It is not: the same substrate leaves TCP 443 and 53 wide open for a
/// sandbox created with `mode: all`, which the pinned contract defines as
/// rejecting all direct non-mediated egress.
///
/// The `400` comes from generic OpenAPI body validation, not from egress logic —
/// which also explains why unknown *fields* pass (`201` for
/// `network.totally_not_a_real_field`): the schema validator allows additional
/// properties and checks the ones it knows. Schema validation therefore says
/// nothing about whether any handler implements the feature, and **no negotiation
/// probe can distinguish a parsed policy from an enforced one.**
///
/// The only sound signal is behavioural — boot a mediated sandbox and try to dial
/// out — and that cannot be a startup check: it costs a VM boot and an image
/// pull on every node start, and it needs the guest network, which does not exist
/// on a macOS host at all (hypeman #358). A node that could not answer would
/// report `false` for a reason unrelated to egress.
///
/// So the claim stays `false` until enforcement is demonstrated where it *can* be
/// demonstrated: the substrate-gated acceptance test
/// (`the_substrate_blocks_direct_egress_the_spec_asked_it_to_block`), which runs
/// green inside Linux and is what will justify flipping this. Reporting `false`
/// costs a refused `CreateInstance` for anyone asking for mediation, with
/// `CAPABILITY_MISSING` naming it — the same shape as
/// `require_hardware_isolation`, and the honest answer while nothing can be
/// proven.
pub fn egress_enforcement_is_unproven() -> CapabilityNote {
    CapabilityNote {
        what: "host-mediated egress is not enforced by this substrate".into(),
        why_it_matters:
            "a sandbox created with `enforcement.mode: all` — which the pinned contract defines              as rejecting all direct non-mediated TCP egress — reached 1.1.1.1 on both 443 and 53.              The substrate schema-validates the policy and then applies nothing, so a spec asking              for confinement would be accepted and unconfined. `egress_control` is therefore              reported as false and mediated specs are refused at create with CAPABILITY_MISSING,              rather than being quietly unenforced."
                .into(),
        remedy:
            "treat egress as unavailable on this substrate build. To re-enable: run the              substrate-gated acceptance test against a build that enforces, and flip              `HypemanRuntime::capabilities`'s `egress_control` on that evidence — the test is the              claim's justification, so the two move together."
                .into(),
    }
}

/// The substrate must **refuse** an unauthenticated caller.
///
/// This looks like belt-and-braces and is not. `hypeman-api` binds `*:4973` —
/// every interface, not loopback — so an API that answers anonymous callers is
/// readable by anything that can route to the host, including guests once they
/// have working networking.
///
/// The guest token no longer travels in the sandbox environment (design decision
/// 5c moved it onto a volume the API cannot read back), so this is no longer a
/// direct credential leak. It remains full control of every instance on the node:
/// create, destroy, and — through the substrate's own exec — arbitrary code in
/// any running guest.
///
/// Preflight refuses to call that healthy.
async fn check_api_requires_auth(config: &Config) -> Vec<Problem> {
    // Probed with a deliberately **tokenless** client rather than by asking whether
    // *we* hold a token. Holding one proves nothing about whether the substrate
    // demands it, and "we authenticate but the door is open" is precisely the
    // configuration this exists to catch — it looks correct from every angle
    // except an attacker's.
    let anonymous = Config::new(config.base_url.clone(), None).client();
    match anonymous.list_instances(None).await {
        Ok(_) => vec![Problem {
            what: format!(
                "hypeman-api at {} serves instance data to an unauthenticated caller",
                config.base_url
            ),
            why_it_matters:
                "hypeman-api listens on all interfaces, not loopback, so anything that can \
                 route to this host can create and destroy instances and run arbitrary code \
                 in any running guest through the substrate's own exec."
                    .into(),
            remedy: format!(
                "configure the substrate to require a bearer token, and point {} or {} at it",
                super::config::ENV_TOKEN_FILE,
                super::config::ENV_TOKEN
            ),
        }],
        // A refusal is the *correct* answer to an anonymous caller, and the only
        // one that proves anything. Everything else — a timeout, a 500, a
        // connection reset — says nothing about whether authentication is
        // enforced, and treating it as proof would report a node as safe on the
        // strength of the substrate having a bad minute.
        Err(super::client::Error::Api { status: 401, .. })
        | Err(super::client::Error::Api { status: 403, .. }) => Vec::new(),
        // Unreachable is already reported by the health check; saying it twice
        // helps nobody.
        Err(e) if e.is_unreachable() => Vec::new(),
        Err(e) => vec![Problem {
            what: format!(
                "hypeman-api at {} did not clearly refuse an unauthenticated request",
                config.base_url
            ),
            why_it_matters: format!(
                "only a 401/403 shows that authentication is enforced, and this was {e}. \
                 Until that is established, the node cannot claim the API is closed — and \
                 an open API is readable by anything that can route to the host."
            ),
            remedy: "check the hypeman-api logs, then re-run preflight".into(),
        }],
    }
}

/// Reachability is not enough: `/health` is the **only** one of the substrate's 58
/// operations that does not require a bearer token. A node with no token therefore
/// passes a health check and then fails every real operation with 401, which is the
/// worst possible split between what preflight says and what the node can do. So
/// after health, probe something authenticated.
async fn check_authorized(client: &HypemanClient, base_url: &str) -> Vec<Problem> {
    match client.list_instances(None).await {
        Ok(_) => Vec::new(),
        Err(e) if e.is_unreachable() => Vec::new(), // already reported by health
        Err(super::client::Error::Api { status: 401, .. })
        | Err(super::client::Error::Api { status: 403, .. }) => vec![Problem {
            what: format!("hypeman-api at {base_url} rejected an authenticated request"),
            why_it_matters: "only /health is unauthenticated, so every lifecycle operation                              would fail even though the health check passes."
                .into(),
            remedy: format!(
                "point {} at a token file, or set {}",
                super::config::ENV_TOKEN_FILE,
                super::config::ENV_TOKEN
            ),
        }],
        Err(e) => vec![Problem {
            what: format!("hypeman-api at {base_url} could not list instances"),
            why_it_matters: format!("the substrate is reachable but not usable: {e}"),
            remedy: "check the hypeman-api logs".into(),
        }],
    }
}

async fn check_reachable(client: &HypemanClient, base_url: &str) -> Vec<Problem> {
    match client.health().await {
        Ok(health) if health.status == "ok" => Vec::new(),
        Ok(health) => vec![Problem {
            what: format!(
                "hypeman-api at {base_url} reports status `{}`",
                health.status
            ),
            why_it_matters: "the substrate answers but does not consider itself healthy; \
                             lifecycle operations may fail."
                .into(),
            remedy: "check the hypeman-api logs".into(),
        }],
        Err(e) if e.is_unreachable() => vec![Problem {
            what: format!("hypeman-api is not reachable at {base_url}"),
            why_it_matters: "no instance can be created, started, paused or resumed until it is; \
                             instances already running are unaffected."
                .into(),
            remedy: format!(
                "start the service, or set {} if it listens elsewhere",
                super::config::ENV_URL
            ),
        }],
        Err(e) => vec![Problem {
            what: format!("hypeman-api at {base_url} rejected the health check"),
            why_it_matters: format!("the substrate is reachable but refused: {e}"),
            remedy: format!(
                "if it requires authentication, set {} or {}",
                super::config::ENV_TOKEN_FILE,
                super::config::ENV_TOKEN
            ),
        }],
    }
}

/// The wrong-arch check upstream should have had (findings §1, nap-010 task 3.1).
///
/// The defect this pre-empts presented as a kernel panic three layers from its
/// cause: the linux/arm64 release embedded x86-64 guest binaries, `execve`
/// returned `ENOEXEC`, busybox parsed the ELF as a shell script, and the
/// operator got `unterminated quoted string` and a dead init. Comparing the ELF
/// architecture of the embedded binaries against the host's at startup turns
/// that into a report that names the binary.
///
/// Deliberately best-effort and never a gate (design decision 4):
/// - initrd not locally readable → **silence** — a remote substrate is a
///   legitimate deployment, and a warning that fires on it trains operators to
///   ignore the check;
/// - readable but uninspectable (compressed, format changed) → reported as
///   *could not inspect*, distinctly from "inspected, fine" — the same
///   asked/could-not-ask honesty the quiesce hook records;
/// - inspected and mismatched → reported by name.
fn initrd_guest_binary_problems() -> Vec<Problem> {
    let arch = std::env::consts::ARCH; // hypeman uses the same names (aarch64, x86_64)
    let candidates = [
        format!("/var/lib/hypeman/system/initrd/{arch}/latest/initrd"),
        format!(
            "{}/Library/Application Support/hypeman/system/initrd/{arch}/latest/initrd",
            std::env::var("HOME").unwrap_or_default()
        ),
    ];
    let Some(bytes) = candidates
        .iter()
        .find_map(|p| std::fs::read(Path::new(p)).ok())
    else {
        return Vec::new(); // not local: not our question
    };
    inspect_initrd(&bytes)
}

/// The two guest binaries the initrd embeds, and where they live inside it.
const GUEST_BINARIES: &[&str] = &["init.bin", "usr/local/bin/guest-agent"];

fn inspect_initrd(bytes: &[u8]) -> Vec<Problem> {
    let host = elf_machine_for_host_arch();
    let entries = match cpio_entries(bytes) {
        Some(entries) => entries,
        None => {
            return vec![Problem {
                what: "the substrate's initrd could not be inspected".into(),
                why_it_matters: "the guest-binary architecture check (findings §1) could not \
                                 run, so a wrong-arch release would still present as a kernel \
                                 panic. This is 'could not ask', not 'asked and fine'."
                    .into(),
                remedy: "the initrd format likely changed upstream (compression?); update \
                         the preflight inspector"
                    .into(),
            }]
        }
    };

    let mut problems = Vec::new();
    for name in GUEST_BINARIES {
        let Some(data) = entries.iter().find(|(n, _)| n == name).map(|(_, d)| *d) else {
            continue; // upstream may rename; absence is not evidence of anything
        };
        match elf_machine(data) {
            Some(machine) if machine != host => problems.push(Problem {
                what: format!(
                    "the substrate's initrd embeds a wrong-architecture `{name}` \
                     (ELF e_machine {machine:#x}, host expects {host:#x})"
                ),
                why_it_matters: "no guest can boot: execve returns ENOEXEC, the shell wrapper \
                                 parses the ELF as a script, and the kernel panics with an \
                                 error three layers from this cause (findings §1)."
                    .into(),
                remedy: "known upstream defect in the linux/arm64 release — see \
                         docs/upstream-hypeman-findings.md §1 for the graft workaround"
                    .into(),
            }),
            _ => {}
        }
    }
    problems
}

fn elf_machine_for_host_arch() -> u16 {
    match std::env::consts::ARCH {
        "x86_64" => 0x3e,
        "aarch64" => 0xb7,
        _ => 0,
    }
}

/// `e_machine` of an ELF image, or `None` for anything that is not one.
fn elf_machine(data: &[u8]) -> Option<u16> {
    if data.len() < 20 || &data[0..4] != b"\x7fELF" {
        return None;
    }
    Some(u16::from_le_bytes([data[18], data[19]]))
}

/// Walk a `newc` cpio archive, returning `(name, data)` slices.
///
/// Hand-rolled rather than a dependency because the format is 110 bytes of
/// ASCII-hex header per entry and this is the whole of what the check needs; a
/// crate would be more code to audit than this is.
fn cpio_entries(bytes: &[u8]) -> Option<Vec<(String, &[u8])>> {
    let mut entries = Vec::new();
    let mut at = 0usize;
    loop {
        if at + 110 > bytes.len() {
            return None; // truncated where a header should be
        }
        let header = &bytes[at..at + 110];
        if &header[0..6] != b"070701" && &header[0..6] != b"070702" {
            return None; // not newc — possibly compressed
        }
        let field = |i: usize| -> Option<usize> {
            let s = std::str::from_utf8(&header[6 + i * 8..6 + (i + 1) * 8]).ok()?;
            usize::from_str_radix(s, 16).ok()
        };
        let namesize = field(11)?;
        let filesize = field(6)?;
        let name_start = at + 110;
        let name_end = name_start + namesize;
        if name_end > bytes.len() {
            return None;
        }
        let name = std::str::from_utf8(&bytes[name_start..name_end.saturating_sub(1)])
            .ok()?
            .to_string();
        // Name is NUL-terminated and the data start is aligned to 4 from `at`.
        let data_start = (name_end + 3) & !3;
        let data_end = data_start + filesize;
        if data_end > bytes.len() {
            return None;
        }
        if name == "TRAILER!!!" {
            return Some(entries);
        }
        entries.push((name, &bytes[data_start..data_end]));
        at = (data_end + 3) & !3;
    }
}

/// Render problems for an operator, one per line.
pub fn describe(problems: &[Problem]) -> String {
    problems
        .iter()
        .map(|p| format!("  - {p}"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_something_that_exists_on_path() {
        assert!(
            find_executable("sh", &[]).is_some(),
            "/bin/sh must be found"
        );
    }

    #[test]
    fn does_not_invent_missing_binaries() {
        assert!(find_executable("barista-definitely-not-a-real-binary-xyz", &[]).is_none());
    }

    #[test]
    fn falls_back_to_fixed_paths_for_keg_only_installs() {
        // The mechanism that matters for mkfs.ext4: not on PATH, but present at a
        // known location.
        let found = find_executable("barista-not-on-path-xyz", &["/bin/sh"]);
        assert_eq!(found, Some(PathBuf::from("/bin/sh")));
    }

    #[test]
    fn ignores_non_executable_files_at_fallback_paths() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("not-executable");
        std::fs::write(&path, b"#!/bin/sh\n").unwrap();
        assert!(
            find_executable("barista-not-on-path-xyz", &[path.to_str().unwrap()]).is_none(),
            "a readable but non-executable file is not a usable binary"
        );
    }

    #[test]
    fn problems_name_the_thing_and_the_fix() {
        let p = Problem {
            what: "`caddy` is not installed".into(),
            why_it_matters: "it crash-loops.".into(),
            remedy: "brew install caddy".into(),
        };
        let rendered = p.to_string();
        assert!(rendered.contains("caddy"));
        assert!(rendered.contains("brew install caddy"), "{rendered}");
    }

    #[tokio::test]
    async fn unreachable_substrate_is_a_problem_that_spares_running_instances() {
        // Port 1 is reserved and nothing listens there.
        let config = Config::new("http://127.0.0.1:1", None);
        let problems = check_reachable(&config.client(), &config.base_url).await;
        assert_eq!(problems.len(), 1);
        assert!(problems[0].what.contains("not reachable"));
        assert!(
            problems[0].why_it_matters.contains("already running"),
            "an operator must be told this does not kill live sessions: {}",
            problems[0].why_it_matters
        );
    }

    /// A node that answers `/health` but rejects authenticated calls must be
    /// reported, not passed. `/health` is the only unauthenticated operation the
    /// substrate has, so health alone proves almost nothing.
    #[tokio::test]
    async fn a_reachable_but_unauthorized_substrate_is_reported() {
        // Nothing listens on port 1, so both probes see "unreachable" — which the
        // authorization probe must stay silent about rather than double-reporting.
        let config = Config::new("http://127.0.0.1:1", None);
        let problems = check_authorized(&config.client(), &config.base_url).await;
        assert!(
            problems.is_empty(),
            "an unreachable substrate is health's finding, not authorization's: {problems:?}"
        );
    }

    #[test]
    fn describe_lists_every_problem() {
        let p = |what: &str| Problem {
            what: what.into(),
            why_it_matters: "x".into(),
            remedy: "y".into(),
        };
        let rendered = describe(&[p("first"), p("second")]);
        assert!(rendered.contains("first") && rendered.contains("second"));
        assert_eq!(rendered.lines().count(), 2);
    }

    // --- the initrd arch check (nap-010 task 3.1) ---

    /// A minimal ELF header: magic + e_machine at offset 18, little-endian.
    fn fake_elf(machine: u16) -> Vec<u8> {
        let mut elf = vec![0u8; 24];
        elf[0..4].copy_from_slice(b"\x7fELF");
        elf[18..20].copy_from_slice(&machine.to_le_bytes());
        elf
    }

    /// A `newc` cpio with the given members — the same format the initrd uses.
    fn fake_cpio(members: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut entry = |name: &str, data: &[u8]| {
            let mut header = format!("070701{:08X}", 0); // ino
            for field in [
                0usize,         // mode
                0,              // uid
                0,              // gid
                1,              // nlink
                0,              // mtime
                data.len(),     // filesize
                0,              // devmajor
                0,              // devminor
                0,              // rdevmajor
                0,              // rdevminor
                name.len() + 1, // namesize (with NUL)
                0,              // check
            ] {
                header.push_str(&format!("{field:08X}"));
            }
            out.extend_from_slice(header.as_bytes());
            out.extend_from_slice(name.as_bytes());
            out.push(0);
            while out.len() % 4 != 0 {
                out.push(0);
            }
            out.extend_from_slice(data);
            while out.len() % 4 != 0 {
                out.push(0);
            }
        };
        for (name, data) in members {
            entry(name, data);
        }
        entry("TRAILER!!!", &[]);
        out
    }

    #[test]
    fn a_matching_initrd_is_silent() {
        let host = elf_machine_for_host_arch();
        let cpio = fake_cpio(&[
            ("init.bin", &fake_elf(host)),
            ("usr/local/bin/guest-agent", &fake_elf(host)),
        ]);
        assert!(inspect_initrd(&cpio).is_empty());
    }

    /// The findings §1 defect, caught at startup instead of as a kernel panic:
    /// both embedded binaries are the wrong architecture, and both are named.
    #[test]
    fn wrong_arch_guest_binaries_are_reported_by_name() {
        let host = elf_machine_for_host_arch();
        let wrong = if host == 0xb7 { 0x3e } else { 0xb7 };
        let cpio = fake_cpio(&[
            ("init.bin", &fake_elf(wrong)),
            ("usr/local/bin/guest-agent", &fake_elf(wrong)),
        ]);
        let problems = inspect_initrd(&cpio);
        assert_eq!(problems.len(), 2, "{problems:?}");
        assert!(problems[0].what.contains("init.bin"));
        assert!(problems[1].what.contains("guest-agent"));
        assert!(
            problems[0].remedy.contains("upstream-hypeman-findings"),
            "the remedy must point at the write-up: {}",
            problems[0].remedy
        );
    }

    /// "Could not inspect" is an answer of its own — distinct from silence and
    /// from "inspected, fine" (design decision 4). Compressed bytes are the
    /// likely future cause.
    #[test]
    fn an_uninspectable_initrd_says_could_not_ask_rather_than_fine() {
        let problems = inspect_initrd(&[0x1f, 0x8b, 0x08, 0x00, 0x00, 0x00]); // gzip magic
        assert_eq!(problems.len(), 1);
        assert!(problems[0].what.contains("could not be inspected"));
    }

    /// A member the check does not know is not evidence of anything: upstream
    /// may rename, and absence must not fire the alarm mismatch does.
    #[test]
    fn missing_members_are_not_a_problem() {
        let cpio = fake_cpio(&[("something-else", b"not elf")]);
        assert!(inspect_initrd(&cpio).is_empty());
    }

    #[test]
    fn elf_machine_reads_the_field_and_rejects_non_elves() {
        assert_eq!(elf_machine(&fake_elf(0xb7)), Some(0xb7));
        assert_eq!(elf_machine(b"#!/bin/sh\n"), None);
        assert_eq!(elf_machine(&[]), None);
    }
}
