//! A local, content-addressed immutable-object store (barista-046 §2.3).
//!
//! Capsule bytes live here as blobs named by their own sha256. The store gives
//! four guarantees the capsule and GC machinery build on:
//!
//! 1. **Verified visibility.** A blob is only ever visible under its final name
//!    after its bytes have been read back and both the length and the digest
//!    match what the manifest claims. A partial or tampered upload never becomes
//!    a readable object (design D3/D4).
//! 2. **Atomic publish.** Staging writes to a temp file in the *same* directory
//!    and the last step is a rename, which is atomic on a POSIX filesystem. A
//!    crash mid-write leaves a staging file, never a half-written object.
//! 3. **Deduplication.** Two capsules that share a blob share one file: `commit`
//!    of an already-present digest is a no-op success. The logical reference
//!    count that decides deletion lives in the journal (`db`), not here — this
//!    layer only owns bytes.
//! 4. **Idempotent, crash-safe reclaim.** `remove` of an absent object succeeds,
//!    and `sweep_staging` deletes leftover staging files from a crashed upload
//!    without touching any committed object.
//!
//! The store is deliberately dumb about *why* a blob exists. Reference counting,
//! GC intents, and the "never delete a live object" rule are the journal's job
//! (`db::decrement_object`, `db::collectable_objects`); this module answers only
//! "are these exact bytes present, and put/remove them safely".

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
// The base trait is referred to fully qualified (`dyn object_store::ObjectStore`)
// because this module's own type is also called `ObjectStore`; only the extension
// trait is imported, and only because `get`/`put`/`head` live on it in 0.14.
use object_store::ObjectStoreExt;
use sha2::{Digest, Sha256};

use crate::capsule::object_digest;

/// Where committed objects and in-flight staging files live.
///
/// `objects/` holds files named by their full `sha256:<hex>` digest (the colon is
/// filesystem-safe on the platforms Barista targets and keeps the on-disk name
/// identical to the wire id, so a stray file is self-describing). `staging/`
/// holds temp files during an upload; nothing there is ever read as an object.
#[derive(Debug)]
pub struct ObjectStore {
    root: PathBuf,
    /// The configured durable tier, or `None` on a node with local storage only
    /// (barista-046 §4.4).
    ///
    /// Absence is not a degraded mode and is never reported as one — the same
    /// rule `fleet` follows for the coordination bucket: a node with no bucket
    /// simply has no remote tier, and says so once, through capabilities. What
    /// makes that honest rather than silent is that a caller asking for the
    /// remote tier gets `OBJECT_STORE_UNAVAILABLE`, not a local write wearing a
    /// remote label.
    remote: Option<Remote>,
}

/// The durable tier: a bucket plus the credential-stripped label an operator
/// needs in a log line.
struct Remote {
    store: std::sync::Arc<dyn object_store::ObjectStore>,
    label: String,
}

/// Deliberately hand-written: the derived form would print the `dyn ObjectStore`
/// (harmless) but invites a future field that is not — a bucket configuration is
/// one `Debug` away from a key in a log.
impl std::fmt::Debug for Remote {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Remote")
            .field("bucket", &self.label)
            .finish()
    }
}

/// A blob written to staging but not yet published. Carries the measured digest
/// and length so the caller can check them against a manifest *before* asking to
/// commit — import must refuse a mismatch, not discover it after publishing.
#[derive(Debug)]
pub struct Staged {
    path: PathBuf,
    pub digest: String,
    pub length: u64,
}

/// True if `digest` is safe to use as a filename under `objects/` — i.e. it is a
/// single path component that cannot escape the directory. Real digests are
/// `sha256:<hex>` and satisfy this trivially; the check exists so a malformed or
/// hostile (manifest-supplied) digest cannot traverse out of the store.
fn is_path_safe_digest(digest: &str) -> bool {
    !digest.is_empty() && !digest.contains(['/', '\\', '\0']) && digest != "." && digest != ".."
}

impl ObjectStore {
    /// Open (creating if absent) a store rooted at `root`. Both subdirectories are
    /// created eagerly so a first upload after a fresh install does not race their
    /// creation.
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        Self::open_with_remote(root, None)
    }

    /// Open a store with an optional durable tier (barista-046 §4.4).
    ///
    /// `remote` is `(store, credential-stripped label)`. The URL grammar and the
    /// credential chain are not re-implemented here: they live in `barista-fleet`
    /// so the node, the CLI, and the gateway all reach a bucket the same way.
    pub fn open_with_remote(
        root: impl AsRef<Path>,
        remote: Option<(std::sync::Arc<dyn object_store::ObjectStore>, String)>,
    ) -> Result<Self> {
        let root = root.as_ref().to_path_buf();
        std::fs::create_dir_all(root.join("objects"))
            .with_context(|| format!("create objects dir under {}", root.display()))?;
        std::fs::create_dir_all(root.join("staging"))
            .with_context(|| format!("create staging dir under {}", root.display()))?;
        Ok(Self {
            root,
            remote: remote.map(|(store, label)| Remote { store, label }),
        })
    }

    /// Whether a durable tier is configured. This is a **node** fact, not a
    /// substrate one — where bytes are stored is the node's decision, so it is
    /// the node that answers for `object_store_snapshots` rather than the
    /// runtime, which owns only whether it can produce and consume the bytes.
    pub fn has_remote(&self) -> bool {
        self.remote.is_some()
    }

    /// The configured bucket, credentials stripped, for an operator-facing line.
    pub fn remote_label(&self) -> Option<&str> {
        self.remote.as_ref().map(|r| r.label.as_str())
    }

    fn remote(&self) -> Result<&Remote> {
        self.remote.as_ref().ok_or_else(|| {
            anyhow::anyhow!(
                "no object-store tier is configured on this node (set the capsule bucket URL)"
            )
        })
    }

    /// Where an object lives in the bucket. Content-addressed, so the key *is*
    /// the digest: two nodes exporting the same object write the same key, which
    /// is what makes the tier deduplicating across a fleet rather than per node.
    fn remote_path(digest: &str) -> object_store::path::Path {
        object_store::path::Path::from(format!("capsules/objects/{digest}"))
    }

    fn object_path(&self, digest: &str) -> PathBuf {
        self.root.join("objects").join(digest)
    }

    /// Is this exact object present and committed?
    pub fn contains(&self, digest: &str) -> bool {
        is_path_safe_digest(digest) && self.object_path(digest).is_file()
    }

    /// Stream `reader` into a staging file, measuring length and digest as the
    /// bytes land. Does not publish: the object is invisible until [`commit`].
    ///
    /// The digest is computed over exactly the bytes written, so a `Staged` can
    /// never claim a digest the file does not have — the only way to lie would be
    /// to write the file out from under the store.
    ///
    /// [`commit`]: ObjectStore::commit
    pub fn stage(&self, mut reader: impl Read) -> Result<Staged> {
        // A unique staging name: the process time in nanos plus a counter is
        // enough because staging files are private to this store and swept on
        // recovery — collisions only cost a retry, never correctness.
        let staging = self.root.join("staging").join(format!(
            "up-{}-{}",
            std::process::id(),
            unique_suffix()
        ));

        let mut file = std::fs::File::create(&staging)
            .with_context(|| format!("create staging file {}", staging.display()))?;
        let mut hasher = Sha256::new();
        let mut length: u64 = 0;
        let mut buf = [0u8; 64 * 1024];
        loop {
            let n = reader.read(&mut buf).context("read object source")?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            file.write_all(&buf[..n]).context("write staging file")?;
            length += n as u64;
        }
        // Durable before we let anyone commit it: a fsync here means a crash after
        // commit's rename cannot leave a named object whose bytes never reached
        // the platter.
        file.sync_all().context("fsync staging file")?;

        let digest = format!("sha256:{}", crate::hex::to_lower(&hasher.finalize()));
        Ok(Staged {
            path: staging,
            digest,
            length,
        })
    }

    /// Convenience: stage from an in-memory buffer. Used by export paths that
    /// already hold the bytes and by tests.
    pub fn stage_bytes(&self, bytes: &[u8]) -> Result<Staged> {
        self.stage(bytes)
    }

    /// Publish a staged blob under `expected_digest`, refusing unless the measured
    /// digest and length match what the manifest claims.
    ///
    /// Idempotent by construction: if the object is already committed the staging
    /// file is discarded and the call succeeds — that is exactly the dedup case
    /// and the retry-after-crash case, which are indistinguishable and both fine.
    pub fn commit(
        &self,
        staged: Staged,
        expected_digest: &str,
        expected_length: u64,
    ) -> Result<()> {
        if staged.digest != expected_digest {
            let _ = std::fs::remove_file(&staged.path);
            bail!(
                "object digest mismatch: manifest claims {expected_digest}, bytes hash to {}",
                staged.digest
            );
        }
        if staged.length != expected_length {
            let _ = std::fs::remove_file(&staged.path);
            bail!(
                "object length mismatch for {expected_digest}: manifest claims {expected_length}, staged {} bytes",
                staged.length
            );
        }

        let dest = self.object_path(expected_digest);
        if dest.is_file() {
            // Already present: shared object, or a replayed import. Drop the
            // duplicate upload rather than rename over a good file.
            let _ = std::fs::remove_file(&staged.path);
            return Ok(());
        }
        // Atomic publish: rename within the same directory tree. After this the
        // object is visible under its verified name and never before.
        std::fs::rename(&staged.path, &dest)
            .with_context(|| format!("publish object {expected_digest}"))?;
        Ok(())
    }

    /// Publish a staged blob to the **durable tier**, and prove it landed
    /// (barista-046 §4.4).
    ///
    /// The spec's wording is the whole design of this function: a snapshot is not
    /// labelled remote "until every required object is durably stored *and
    /// verified*". So the upload is not the last step — the bytes are read back
    /// out of the bucket and re-hashed. A silently truncated PUT, a bucket that
    /// accepted the write and dropped it, or a mid-flight corruption all fail
    /// here, before anything is registered as remote.
    ///
    /// Staging is local for both tiers on purpose: measuring the digest as bytes
    /// land is guarantee #1, and it does not become weaker because the
    /// destination is a bucket. It also means a remote commit needs no multipart
    /// resumability to be crash-safe — a crash leaves a local staging file that
    /// the startup sweep collects.
    pub async fn commit_remote(
        &self,
        staged: Staged,
        expected_digest: &str,
        expected_length: u64,
    ) -> Result<()> {
        let remote = self.remote()?;
        // The same refusals as the local path, and for the same reason: a caller
        // that lied about the digest must not reach storage at all.
        if staged.digest != expected_digest {
            let _ = std::fs::remove_file(&staged.path);
            bail!(
                "object digest mismatch: manifest claims {expected_digest}, bytes hash to {}",
                staged.digest
            );
        }
        if staged.length != expected_length {
            let _ = std::fs::remove_file(&staged.path);
            bail!(
                "object length mismatch for {expected_digest}: manifest claims {expected_length}, staged {} bytes",
                staged.length
            );
        }

        let path = Self::remote_path(expected_digest);
        // Already durable: a shared object or a replayed export. Nothing to
        // upload, and re-uploading would only risk replacing a good object.
        if !self.remote_contains(expected_digest).await? {
            let bytes = std::fs::read(&staged.path)
                .with_context(|| format!("read staged object {expected_digest}"))?;
            remote
                .store
                .put(&path, bytes.into())
                .await
                .with_context(|| format!("upload object {expected_digest} to {}", remote.label))?;

            // Read back and re-hash. This is the "and verified" half.
            let round_tripped = remote
                .store
                .get(&path)
                .await
                .with_context(|| format!("read back object {expected_digest}"))?
                .bytes()
                .await
                .with_context(|| format!("read back object {expected_digest}"))?;
            let actual = object_digest(&round_tripped);
            if actual != expected_digest {
                // Leave nothing readable behind that we could not verify: a
                // bucket object under a digest it does not hash to would be a
                // trap for every later reader.
                let _ = remote.store.delete(&path).await;
                let _ = std::fs::remove_file(&staged.path);
                bail!(
                    "object {expected_digest} did not survive the round trip to {}: it reads back \
                     as {actual}",
                    remote.label
                );
            }
        }

        // The local staging file has served its purpose. The local *object* tier
        // is left alone: a node that also holds the bytes locally is a cache hit
        // for the next restore, not a duplicate to clean up.
        let _ = std::fs::remove_file(&staged.path);
        Ok(())
    }

    /// Is this object durably present in the configured tier?
    pub async fn remote_contains(&self, digest: &str) -> Result<bool> {
        let remote = self.remote()?;
        match remote.store.head(&Self::remote_path(digest)).await {
            Ok(_) => Ok(true),
            Err(object_store::Error::NotFound { .. }) => Ok(false),
            Err(e) => Err(e).with_context(|| format!("check object {digest} in {}", remote.label)),
        }
    }

    /// Read an object's bytes wherever they are: the local tier first, then the
    /// durable one, caching a remote hit locally on the way through.
    ///
    /// This is what lets a restore work on a node that never held the bytes —
    /// the spec's "restores after source loss" — and the local cache is what
    /// keeps a second restore from paying for the download twice. Both paths
    /// verify: local through [`read_verified`], remote by re-hashing before the
    /// bytes are published locally.
    ///
    /// [`read_verified`]: ObjectStore::read_verified
    pub async fn fetch(&self, digest: &str) -> Result<Option<Vec<u8>>> {
        if let Some(bytes) = self.read_verified(digest)? {
            return Ok(Some(bytes));
        }
        let Some(remote) = self.remote.as_ref() else {
            return Ok(None);
        };
        let bytes = match remote.store.get(&Self::remote_path(digest)).await {
            Ok(got) => got
                .bytes()
                .await
                .with_context(|| format!("download object {digest} from {}", remote.label))?,
            Err(object_store::Error::NotFound { .. }) => return Ok(None),
            Err(e) => {
                return Err(e)
                    .with_context(|| format!("download object {digest} from {}", remote.label))
            }
        };
        let actual = object_digest(&bytes);
        if actual != digest {
            bail!(
                "object {digest} in {} is corrupt: its bytes hash to {actual}",
                remote.label
            );
        }
        // Cache it locally through the same verify-then-publish path every other
        // write takes, so a downloaded object is indistinguishable from an
        // exported one and the next reader pays nothing.
        let staged = self.stage(bytes.as_ref())?;
        self.commit(staged, digest, bytes.len() as u64)?;
        Ok(Some(bytes.to_vec()))
    }

    /// Remove an object from the durable tier. Idempotent, like its local twin:
    /// a retried GC pass must not fail on bytes a previous pass collected.
    pub async fn remove_remote(&self, digest: &str) -> Result<()> {
        let remote = self.remote()?;
        match remote.store.delete(&Self::remote_path(digest)).await {
            Ok(()) => Ok(()),
            Err(object_store::Error::NotFound { .. }) => Ok(()),
            Err(e) => {
                Err(e).with_context(|| format!("remove object {digest} from {}", remote.label))
            }
        }
    }

    /// Open a committed object for reading, re-verifying nothing: an object under
    /// its name was verified at commit and is immutable, so callers pay the digest
    /// cost once. `None` if it is not present.
    pub fn open_object(&self, digest: &str) -> Result<Option<std::fs::File>> {
        // The digest is joined into a path; a manifest-supplied value like
        // `../../x` must never escape `objects/`. Real digests are `sha256:<hex>`
        // and contain no separator, so this only fires on malformed or hostile
        // input — closing traversal without relying on verify-before-use ordering.
        if !is_path_safe_digest(digest) {
            bail!("refusing to open object with unsafe digest {digest:?}");
        }
        let path = self.object_path(digest);
        match std::fs::File::open(&path) {
            Ok(f) => Ok(Some(f)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| format!("open object {digest}")),
        }
    }

    /// Read a committed object's full bytes, re-checking the digest. Used where a
    /// caller wants to be paranoid (verify-on-read); export/import rely on the
    /// commit-time check for the hot path.
    pub fn read_verified(&self, digest: &str) -> Result<Option<Vec<u8>>> {
        let Some(mut f) = self.open_object(digest)? else {
            return Ok(None);
        };
        let mut bytes = Vec::new();
        f.read_to_end(&mut bytes)
            .with_context(|| format!("read object {digest}"))?;
        let actual = object_digest(&bytes);
        if actual != digest {
            bail!("object {digest} is corrupt on disk: bytes now hash to {actual}");
        }
        Ok(Some(bytes))
    }

    /// Physically remove a committed object. Idempotent: removing an absent object
    /// succeeds, so a retried GC pass never fails on a blob a previous pass already
    /// collected.
    ///
    /// This layer does **not** decide whether removal is safe — the journal's
    /// reference count does (design D6). Callers must have a durable GC intent and
    /// a zero reference count before calling this.
    pub fn remove(&self, digest: &str) -> Result<()> {
        if !is_path_safe_digest(digest) {
            bail!("refusing to remove object with unsafe digest {digest:?}");
        }
        match std::fs::remove_file(self.object_path(digest)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e).with_context(|| format!("remove object {digest}")),
        }
    }

    /// Delete every leftover staging file. Run on startup: a staging file can only
    /// exist because an upload crashed before commit renamed it out, so none of
    /// them is a live object and all are safe to drop (design D6, crash recovery).
    /// Returns how many were swept, for the reconciliation log.
    pub fn sweep_staging(&self) -> Result<usize> {
        let dir = self.root.join("staging");
        let mut swept = 0;
        for entry in std::fs::read_dir(&dir).with_context(|| format!("scan {}", dir.display()))? {
            let entry = entry.context("read staging entry")?;
            if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                std::fs::remove_file(entry.path())
                    .with_context(|| format!("sweep staging file {}", entry.path().display()))?;
                swept += 1;
            }
        }
        Ok(swept)
    }
}

/// A monotonic-ish suffix for staging names. Nanos since an arbitrary epoch plus a
/// process-local counter: uniqueness only has to hold within one store's staging
/// dir, and a collision costs a `create` retry, never a wrong object.
fn unique_suffix() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{nanos:x}-{n:x}")
}

/// Reconcile physical object bytes with the journal's decisions (barista-046
/// §2.4/2.5). Run on node startup and after a delete: it is the retryable
/// physical half of design D6.
///
/// Two independent, both crash-safe, both idempotent steps:
///
/// 1. **Sweep staging.** A staging file is the only trace a crashed upload
///    leaves and is never a live object, so all are dropped.
/// 2. **Collect unreferenced objects.** For each object the journal has marked
///    collectable (`refcount == 0` and a durable GC intent), remove the bytes
///    and then finalize the journal row. The journal is consulted *first* and
///    `finalize_object_gc` re-checks the count, so an object that a concurrent
///    import re-referenced between the query and here is never deleted — the
///    "never remove an object with a live reference" invariant holds across the
///    race.
///
/// Returns `(staging_swept, objects_collected)` for the reconciliation log.
pub fn run_gc(db: &crate::db::Db, store: &ObjectStore) -> Result<(usize, usize)> {
    let swept = store.sweep_staging()?;
    let mut collected = 0;
    for digest in db.collectable_objects()? {
        // Remove bytes first, then finalize the journal. If we crash between the
        // two, the object row survives with its GC intent and a later pass
        // re-runs both steps — `remove` is idempotent, so the retry is free.
        store.remove(&digest)?;
        db.finalize_object_gc(&digest)?;
        collected += 1;
    }
    Ok((swept, collected))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capsule::object_digest;

    fn store() -> (ObjectStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        (ObjectStore::open(dir.path()).unwrap(), dir)
    }

    #[test]
    fn stage_measures_digest_and_length() {
        let (s, _d) = store();
        let staged = s.stage_bytes(b"hello world").unwrap();
        assert_eq!(staged.digest, object_digest(b"hello world"));
        assert_eq!(staged.length, 11);
        // Not visible until committed.
        assert!(!s.contains(&staged.digest));
    }

    #[test]
    fn commit_publishes_and_reads_back() {
        let (s, _d) = store();
        let bytes = b"the payload";
        let staged = s.stage_bytes(bytes).unwrap();
        let digest = staged.digest.clone();
        s.commit(staged, &digest, bytes.len() as u64).unwrap();
        assert!(s.contains(&digest));
        assert_eq!(s.read_verified(&digest).unwrap().unwrap(), bytes);
    }

    #[test]
    fn commit_refuses_a_digest_mismatch() {
        let (s, _d) = store();
        let staged = s.stage_bytes(b"real bytes").unwrap();
        let len = staged.length;
        // Claim someone else's digest: the store must refuse to publish under a
        // name the bytes do not have.
        let err = s
            .commit(staged, "sha256:0000", len)
            .unwrap_err()
            .to_string();
        assert!(err.contains("digest mismatch"), "{err}");
    }

    #[test]
    fn commit_refuses_a_length_mismatch() {
        let (s, _d) = store();
        let staged = s.stage_bytes(b"twelve bytes").unwrap();
        let digest = staged.digest.clone();
        let err = s.commit(staged, &digest, 999).unwrap_err().to_string();
        assert!(err.contains("length mismatch"), "{err}");
    }

    #[test]
    fn commit_is_idempotent_and_dedups() {
        let (s, _d) = store();
        let bytes = b"shared blob";
        let digest = object_digest(bytes);

        s.commit(s.stage_bytes(bytes).unwrap(), &digest, bytes.len() as u64)
            .unwrap();
        // A second capsule references the same object: committing again succeeds
        // and does not corrupt the file.
        s.commit(s.stage_bytes(bytes).unwrap(), &digest, bytes.len() as u64)
            .unwrap();
        assert_eq!(s.read_verified(&digest).unwrap().unwrap(), bytes);
    }

    #[test]
    fn remove_is_idempotent() {
        let (s, _d) = store();
        let bytes = b"gone soon";
        let digest = object_digest(bytes);
        s.commit(s.stage_bytes(bytes).unwrap(), &digest, bytes.len() as u64)
            .unwrap();
        s.remove(&digest).unwrap();
        assert!(!s.contains(&digest));
        // Removing again is fine — a retried GC pass must not fail.
        s.remove(&digest).unwrap();
    }

    #[test]
    fn traversal_shaped_digests_cannot_escape_the_store() {
        let (s, _d) = store();
        for bad in [
            "../../etc/passwd",
            "..",
            ".",
            "a/b",
            "sha256:../x",
            "/etc/passwd",
            "",
        ] {
            assert!(!s.contains(bad), "contains must reject {bad:?}");
            assert!(
                s.open_object(bad).is_err(),
                "open_object must reject {bad:?}"
            );
            assert!(s.remove(bad).is_err(), "remove must reject {bad:?}");
        }
    }

    /// Recovery: a staging file is the only trace a crashed upload leaves, and it
    /// is never a live object. `sweep_staging` clears them and touches nothing
    /// committed.
    #[test]
    fn sweep_clears_staging_but_not_objects() {
        let (s, dir) = store();
        let bytes = b"committed";
        let digest = object_digest(bytes);
        s.commit(s.stage_bytes(bytes).unwrap(), &digest, bytes.len() as u64)
            .unwrap();

        // Simulate a crashed upload: a staging file with no matching commit.
        std::fs::write(dir.path().join("staging").join("up-crashed"), b"partial").unwrap();

        assert_eq!(s.sweep_staging().unwrap(), 1);
        assert!(s.contains(&digest), "committed object must survive a sweep");
        assert_eq!(
            std::fs::read_dir(dir.path().join("staging"))
                .unwrap()
                .count(),
            0
        );
    }

    /// End-to-end GC (task 2.4/2.5): the journal and the store agree. An object a
    /// live capsule references is never collected; one whose last capsule is
    /// deleted is swept from disk by `run_gc`, and a shared object survives until
    /// its last reference is gone.
    #[test]
    fn run_gc_collects_only_unreferenced_objects() {
        use crate::db::{CapsuleRow, Db};
        use barista_proto::node::v1alpha1 as pb;

        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::open(dir.path().join("objects-root")).unwrap();
        let db = Db::open(&dir.path().join("j.sqlite3")).unwrap();

        let live = b"kept alive";
        let dead = b"about to die";
        let live_digest = object_digest(live);
        let dead_digest = object_digest(dead);
        store
            .commit(
                store.stage_bytes(live).unwrap(),
                &live_digest,
                live.len() as u64,
            )
            .unwrap();
        store
            .commit(
                store.stage_bytes(dead).unwrap(),
                &dead_digest,
                dead.len() as u64,
            )
            .unwrap();

        let cap = |id: &str, digest: &str, len: u64| CapsuleRow {
            capsule_id: id.into(),
            manifest: pb::CapsuleManifest {
                schema_version: crate::capsule::SCHEMA_VERSION.into(),
                objects: vec![pb::CapsuleObject {
                    digest: digest.into(),
                    length: len,
                    r#type: pb::CapsuleObjectType::Memory as i32,
                    media_type: crate::capsule::media_type(pb::CapsuleObjectType::Memory).into(),
                }],
                ..Default::default()
            },
            storage: pb::CapsuleStorage::LocalDir,
            total_size: len,
            created_at_ms: 0,
        };
        db.register_capsule(&cap("cap-live", &live_digest, live.len() as u64))
            .unwrap();
        db.register_capsule(&cap("cap-dead", &dead_digest, dead.len() as u64))
            .unwrap();

        // Nothing to collect while both are referenced.
        assert_eq!(run_gc(&db, &store).unwrap(), (0, 0));
        assert!(store.contains(&live_digest) && store.contains(&dead_digest));

        // Delete the dead capsule and collect: only its object goes.
        db.delete_capsule("cap-dead").unwrap();
        assert_eq!(run_gc(&db, &store).unwrap(), (0, 1));
        assert!(
            store.contains(&live_digest),
            "a live object must never be collected"
        );
        assert!(
            !store.contains(&dead_digest),
            "an unreferenced object must be swept"
        );
        // The journal row is finalized too, so a second pass is a clean no-op.
        assert_eq!(run_gc(&db, &store).unwrap(), (0, 0));
    }

    // --- the durable tier (barista-046 §4.4) -------------------------------

    use std::sync::Arc;

    /// A store with an in-memory bucket behind it, plus a handle on that bucket
    /// so a test can look at — or tamper with — what actually landed.
    fn with_remote(dir: &tempfile::TempDir) -> (ObjectStore, Arc<object_store::memory::InMemory>) {
        let bucket = Arc::new(object_store::memory::InMemory::new());
        let store = ObjectStore::open_with_remote(
            dir.path(),
            Some((bucket.clone(), "s3://capsules".into())),
        )
        .unwrap();
        (store, bucket)
    }

    /// A remote tier exists only when configured, and that is the fact the node
    /// answers `object_store_snapshots` from.
    #[test]
    fn a_tier_exists_only_when_configured() {
        let dir = tempfile::tempdir().unwrap();
        let local = ObjectStore::open(dir.path()).unwrap();
        assert!(!local.has_remote());
        assert_eq!(local.remote_label(), None);

        let dir2 = tempfile::tempdir().unwrap();
        let (remote, _) = with_remote(&dir2);
        assert!(remote.has_remote());
        assert_eq!(remote.remote_label(), Some("s3://capsules"));
    }

    /// Asking a local-only store to publish remotely fails; it does not quietly
    /// write locally and call it durable.
    #[tokio::test]
    async fn a_remote_commit_without_a_tier_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::open(dir.path()).unwrap();
        let staged = store.stage_bytes(b"bytes").unwrap();
        let (digest, length) = (staged.digest.clone(), staged.length);
        let err = store
            .commit_remote(staged, &digest, length)
            .await
            .expect_err("no tier is configured");
        assert!(
            err.to_string()
                .contains("no object-store tier is configured"),
            "unexpected: {err}"
        );
        assert!(
            !store.contains(&digest),
            "and nothing was published locally"
        );
    }

    /// The flagship property: bytes published by one node are restorable by a
    /// node that never held them locally — the spec's "survives loss of the
    /// source node", proven at the layer that owns it.
    #[tokio::test]
    async fn another_node_fetches_what_this_one_published() {
        let bucket = Arc::new(object_store::memory::InMemory::new());
        let exporter_dir = tempfile::tempdir().unwrap();
        let importer_dir = tempfile::tempdir().unwrap();
        let exporter = ObjectStore::open_with_remote(
            exporter_dir.path(),
            Some((bucket.clone(), "s3://capsules".into())),
        )
        .unwrap();
        let importer = ObjectStore::open_with_remote(
            importer_dir.path(),
            Some((bucket.clone(), "s3://capsules".into())),
        )
        .unwrap();

        let staged = exporter.stage_bytes(b"exact-memory").unwrap();
        let (digest, length) = (staged.digest.clone(), staged.length);
        exporter
            .commit_remote(staged, &digest, length)
            .await
            .unwrap();

        // The importer has never seen these bytes locally.
        assert!(!importer.contains(&digest));
        let got = importer.fetch(&digest).await.unwrap();
        assert_eq!(got.as_deref(), Some(&b"exact-memory"[..]));
        // And it cached them on the way through, so a second restore is local.
        assert!(
            importer.contains(&digest),
            "a remote hit is cached locally, verified, through the normal publish path"
        );
    }

    /// A local hit is answered without the bucket at all.
    #[tokio::test]
    async fn fetch_prefers_the_local_tier() {
        let dir = tempfile::tempdir().unwrap();
        let (store, bucket) = with_remote(&dir);
        let staged = store.stage_bytes(b"local").unwrap();
        let (digest, length) = (staged.digest.clone(), staged.length);
        store.commit(staged, &digest, length).unwrap();

        assert_eq!(
            store.fetch(&digest).await.unwrap().as_deref(),
            Some(&b"local"[..])
        );
        // Never uploaded, so the bucket is untouched — the local tier answered.
        assert!(!store.remote_contains(&digest).await.unwrap());
        let _ = bucket;
    }

    /// A digest that does not describe the bytes never reaches the bucket.
    #[tokio::test]
    async fn a_lying_digest_is_refused_before_upload() {
        let dir = tempfile::tempdir().unwrap();
        let (store, _bucket) = with_remote(&dir);
        let staged = store.stage_bytes(b"real").unwrap();
        let length = staged.length;
        let err = store
            .commit_remote(staged, "sha256:deadbeef", length)
            .await
            .expect_err("a mismatched digest must be refused");
        assert!(
            err.to_string().contains("digest mismatch"),
            "unexpected: {err}"
        );
        assert!(!store.remote_contains("sha256:deadbeef").await.unwrap());
    }

    /// Bytes that rot in the bucket are caught on the way out rather than handed
    /// to a memory restore.
    #[tokio::test]
    async fn a_corrupt_remote_object_is_caught_on_fetch() {
        let dir = tempfile::tempdir().unwrap();
        let (store, bucket) = with_remote(&dir);
        let staged = store.stage_bytes(b"honest").unwrap();
        let (digest, length) = (staged.digest.clone(), staged.length);
        store.commit_remote(staged, &digest, length).await.unwrap();

        // Overwrite the object under its own key with different bytes.
        use object_store::ObjectStoreExt as _;
        bucket
            .put(
                &object_store::path::Path::from(format!("capsules/objects/{digest}")),
                b"tampered".to_vec().into(),
            )
            .await
            .unwrap();

        let err = store
            .fetch(&digest)
            .await
            .expect_err("a corrupt remote object must not be returned");
        assert!(err.to_string().contains("is corrupt"), "unexpected: {err}");
        assert!(
            !store.contains(&digest),
            "and it must not have been cached locally either"
        );
    }

    /// An object the bucket does not have is `None`, not an error: "absent" is a
    /// normal answer a caller has to handle, and it is how an import reports a
    /// capsule it cannot verify.
    #[tokio::test]
    async fn an_absent_remote_object_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let (store, _bucket) = with_remote(&dir);
        assert!(store.fetch("sha256:absent").await.unwrap().is_none());
    }

    /// Publishing an object the bucket already holds is a no-op success — the
    /// dedup case and the retry-after-crash case, indistinguishable and both fine.
    #[tokio::test]
    async fn a_second_remote_commit_is_a_dedup_no_op() {
        let dir = tempfile::tempdir().unwrap();
        let (store, _bucket) = with_remote(&dir);
        let first = store.stage_bytes(b"shared").unwrap();
        let (digest, length) = (first.digest.clone(), first.length);
        store.commit_remote(first, &digest, length).await.unwrap();

        let second = store.stage_bytes(b"shared").unwrap();
        store.commit_remote(second, &digest, length).await.unwrap();
        assert!(store.remote_contains(&digest).await.unwrap());
    }

    /// Removal is idempotent in the durable tier too, so a retried GC pass never
    /// fails on bytes an earlier pass already collected.
    #[tokio::test]
    async fn remote_removal_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let (store, _bucket) = with_remote(&dir);
        let staged = store.stage_bytes(b"doomed").unwrap();
        let (digest, length) = (staged.digest.clone(), staged.length);
        store.commit_remote(staged, &digest, length).await.unwrap();

        store.remove_remote(&digest).await.unwrap();
        assert!(!store.remote_contains(&digest).await.unwrap());
        store.remove_remote(&digest).await.unwrap();
    }
}
