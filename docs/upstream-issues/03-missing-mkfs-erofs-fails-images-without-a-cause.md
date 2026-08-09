# A missing mkfs.erofs fails every image with the cause only in the journal

**Versions:** hypeman-api 0.17.0 (Linux).

Without `erofs-utils` installed, every OCI image conversion lands in
`status: failed` and the API response carries nothing actionable — the actual
cause (`mkfs.erofs: not found`) appears only in the daemon's journal. For
anyone driving the API programmatically, the failure is indistinguishable from
a broken image.

Two cheap fixes, either sufficient:

- a startup preflight that names missing host tools (`mkfs.erofs`,
  `mkfs.ext4`, `caddy`) instead of failing lazily at first use;
- surfacing the conversion error's stderr in the image's `status`/`error`
  field, where the API consumer is already looking.

Suggested install-doc note in the meantime: `apt-get install erofs-utils` is a
hard requirement on Linux.
