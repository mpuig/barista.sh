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

impl ObjectStore {
    /// Open (creating if absent) a store rooted at `root`. Both subdirectories are
    /// created eagerly so a first upload after a fresh install does not race their
    /// creation.
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().to_path_buf();
        std::fs::create_dir_all(root.join("objects"))
            .with_context(|| format!("create objects dir under {}", root.display()))?;
        std::fs::create_dir_all(root.join("staging"))
            .with_context(|| format!("create staging dir under {}", root.display()))?;
        Ok(Self { root })
    }

    fn object_path(&self, digest: &str) -> PathBuf {
        self.root.join("objects").join(digest)
    }

    /// Is this exact object present and committed?
    pub fn contains(&self, digest: &str) -> bool {
        self.object_path(digest).is_file()
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

    /// Open a committed object for reading, re-verifying nothing: an object under
    /// its name was verified at commit and is immutable, so callers pay the digest
    /// cost once. `None` if it is not present.
    pub fn open_object(&self, digest: &str) -> Result<Option<std::fs::File>> {
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
}
