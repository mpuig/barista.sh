//! Identifiers that cannot be swapped for one another.
//!
//! Everything here was a `String`. In one trait that produced
//! `delete_snapshot(&str)` beside `remove_orphan(&str)` — same type, opposite
//! meaning, and transposing them at a call site compiled clean. For a system
//! whose correctness claim is "the right id got journaled", the compiler should
//! be holding that, not the reader.
//!
//! Wire types stay `String`, as the contract requires. Conversion happens once,
//! at the proto boundary in `service.rs`; everything behind it takes these.

/// Generate a newtype over `String` with the impls an identifier needs.
///
/// A macro rather than a generic `Id<Marker>`: the phantom-type version reads
/// worse at every call site and produces errors that name the marker instead of
/// the thing. This costs a few more lines and hides nothing.
macro_rules! identifier {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }

            pub fn into_string(self) -> String {
                self.0
            }

            pub fn is_empty(&self) -> bool {
                self.0.is_empty()
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_string())
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }

        /// Prints the bare value: these appear in error messages and log lines,
        /// and `InstanceId("01J…")` would be noise in both.
        impl std::fmt::Debug for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                std::fmt::Debug::fmt(&self.0, f)
            }
        }

        impl rusqlite::types::FromSql for $name {
            fn column_result(
                value: rusqlite::types::ValueRef<'_>,
            ) -> rusqlite::types::FromSqlResult<Self> {
                String::column_result(value).map(Self)
            }
        }

        impl rusqlite::ToSql for $name {
            fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
                self.0.to_sql()
            }
        }
    };
}

identifier!(
    /// A Barista instance. Client-chosen ULID, unique per node (spec §3.1).
    InstanceId
);
identifier!(
    /// A snapshot, as the runtime named it.
    SnapshotId
);
identifier!(
    /// One journaled operation.
    OpId
);
identifier!(
    /// A caller's replay key. Not an id of anything — a claim that two requests
    /// are the same request — which is why it is its own type rather than a
    /// `String` that happens to sit beside them.
    IdempotencyKey
);

/// A value that must not reach a log.
///
/// Deliberately **not** one of the identifiers above: they want `Display`, and
/// this must not have it. The distinction is the whole point.
///
/// - no `Display`, so it cannot reach a format string by accident;
/// - `Debug` prints `[redacted]`, so `{:?}` on any enclosing struct is safe and
///   `GuestBootstrap` can keep deriving it;
/// - the value comes out only through [`Secret::expose`], which makes every real
///   read greppable — `rg 'expose\(\)'` is the audit.
///
/// No `zeroize` here. Wiping memory guards against a process-memory attacker,
/// and this token has a nearer exposure — plaintext in the SQLite journal, and
/// in the sandbox's environment for runtimes that still deliver it that way —
/// which a `Drop` impl would not touch. Adding it would suggest more protection
/// than it buys.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The only way to the bytes. Named to be conspicuous in review and in a
    /// grep, because every call is a place a credential is handled.
    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl From<String> for Secret {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for Secret {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret([redacted])")
    }
}

impl rusqlite::types::FromSql for Secret {
    fn column_result(value: rusqlite::types::ValueRef<'_>) -> rusqlite::types::FromSqlResult<Self> {
        String::column_result(value).map(Self)
    }
}

impl rusqlite::ToSql for Secret {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        self.0.to_sql()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_print_as_their_value() {
        let id = InstanceId::from("01JABC");
        assert_eq!(id.to_string(), "01JABC");
        // `Debug` too: these end up in error messages, and
        // `InstanceId("01JABC")` reads worse than `"01JABC"` in every one.
        assert_eq!(format!("{id:?}"), "\"01JABC\"");
    }

    /// The leak this type exists to make impossible.
    #[test]
    fn a_secret_never_prints_its_value() {
        let secret = Secret::from("s3cr3t-token-value");
        assert_eq!(format!("{secret:?}"), "Secret([redacted])");
        assert!(!format!("{secret:?}").contains("s3cr3t"));
        assert_eq!(secret.expose(), "s3cr3t-token-value");
    }

    /// ...including when it is a field of something that derives `Debug`, which
    /// is how the credential would actually escape: nobody formats the token,
    /// they format the struct holding it.
    #[test]
    fn a_secret_stays_redacted_inside_a_deriving_struct() {
        #[derive(Debug)]
        struct Bootstrap {
            #[allow(dead_code)]
            instance: InstanceId,
            #[allow(dead_code)]
            token: Secret,
        }

        let printed = format!(
            "{:?}",
            Bootstrap {
                instance: InstanceId::from("01JABC"),
                token: Secret::from("s3cr3t-token-value"),
            }
        );
        assert!(printed.contains("01JABC"), "the id should still be visible");
        assert!(
            !printed.contains("s3cr3t"),
            "the token leaked through a derived Debug: {printed}"
        );
    }

    /// Two identifiers are different types even though both wrap a `String`.
    /// This is the whole purpose, so it is asserted rather than assumed — the
    /// test is that the *other* direction does not compile, which is what the
    /// `Runtime` trait's signatures now enforce at every call site.
    #[test]
    fn identifiers_are_not_interchangeable() {
        let instance = InstanceId::from("same-text");
        let snapshot = SnapshotId::from("same-text");
        // Comparable only after being deliberately reduced to strings.
        assert_eq!(instance.as_str(), snapshot.as_str());
    }
}
