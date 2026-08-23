//! Lowercase hex encoding without a per-byte allocation.
//!
//! The codebase spells byte-wise hex as `bytes.iter().map(|b|
//! format!("{b:02x}")).collect::<String>()` in many places (content ids,
//! template hashes, key fingerprints). That heap-allocates a temporary `String`
//! for **every byte** — 32 allocations for one sha256 digest — on paths that run
//! per staged object and per capsule id. This encodes into a single pre-sized
//! `String` instead, one allocation total, byte-for-byte identical output.
//!
//! The technique mirrors an upstream Rust optimization (openai/codex #38823,
//! "avoid allocating per character"): format into a reused buffer rather than a
//! fresh allocation per element.

const HEX: &[u8; 16] = b"0123456789abcdef";

/// Lowercase hex of `bytes`, e.g. `[0xde, 0xad]` → `"dead"`. One allocation.
///
/// Identical output to `bytes.iter().map(|b| format!("{b:02x}")).collect()`, so
/// it is a drop-in replacement everywhere that pattern appears — including under
/// pinned golden digests, which the `capsule` fixtures verify.
pub fn to_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Byte-for-byte identical to the `format!("{b:02x}")` pattern it replaces —
    /// the property that lets it go under pinned content ids unchanged.
    #[test]
    fn matches_the_format_pattern_it_replaces() {
        for case in [
            [].as_slice(),
            &[0x00],
            &[0xff],
            &[0xde, 0xad, 0xbe, 0xef],
            &[0x01, 0x0a, 0x10, 0xa0, 0x7f, 0x80],
        ] {
            let want: String = case.iter().map(|b| format!("{b:02x}")).collect();
            assert_eq!(to_lower(case), want, "mismatch for {case:?}");
        }
    }

    #[test]
    fn known_vectors() {
        assert_eq!(to_lower(b""), "");
        assert_eq!(to_lower(&[0xca, 0xfe]), "cafe");
        assert_eq!(to_lower(&[0, 1, 2, 255]), "000102ff");
    }
}
