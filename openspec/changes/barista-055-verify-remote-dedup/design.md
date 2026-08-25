# Design: verified remote dedup

A key name and `HEAD` metadata are not trusted integrity evidence. When the immutable key exists, commit fetches its bytes and applies the same length and SHA-256 verification used on restore. A valid object is a dedup hit. An invalid object is removed and the call fails; the staged local object remains available for a clean retry. This avoids overwriting through ambiguous object-store create semantics.
