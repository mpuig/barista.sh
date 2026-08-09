//! Configuration for reaching the local `hypeman-api`.
//!
//! The bearer token is a secret. It is deliberately awkward to leak: `Debug` is
//! hand-written to redact it, and nothing else in this module formats it.

use std::path::Path;

/// Environment variable holding the API base URL.
pub const ENV_URL: &str = "BARISTA_HYPEMAN_URL";
/// Environment variable holding the bearer token directly.
pub const ENV_TOKEN: &str = "BARISTA_HYPEMAN_TOKEN";
/// Environment variable pointing at a file containing the bearer token — the
/// preferred form, because a path does not appear in `ps` output or in a process's
/// environment dump the way a secret does.
pub const ENV_TOKEN_FILE: &str = "BARISTA_HYPEMAN_TOKEN_FILE";

#[derive(Clone, PartialEq, Eq)]
pub struct Config {
    pub base_url: String,
    token: Option<String>,
}

// Hand-written so the token cannot reach a log line through `{:?}` on any struct
// that happens to contain a Config.
impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("base_url", &self.base_url)
            .field(
                "token",
                &match &self.token {
                    Some(_) => "<redacted>",
                    None => "<none>",
                },
            )
            .finish()
    }
}

impl Config {
    pub fn new(base_url: impl Into<String>, token: Option<String>) -> Self {
        Self {
            base_url: base_url.into(),
            token: token.filter(|t| !t.trim().is_empty()),
        }
    }

    /// Read configuration from the environment, preferring a token *file* over an
    /// inline token.
    pub fn from_env() -> anyhow::Result<Self> {
        let base_url =
            std::env::var(ENV_URL).unwrap_or_else(|_| super::client::DEFAULT_BASE_URL.to_string());
        let token = match std::env::var(ENV_TOKEN_FILE) {
            Ok(path) if !path.trim().is_empty() => Some(Self::read_token_file(Path::new(&path))?),
            _ => std::env::var(ENV_TOKEN).ok(),
        };
        Ok(Self::new(base_url, token))
    }

    fn read_token_file(path: &Path) -> anyhow::Result<String> {
        let raw = std::fs::read_to_string(path).map_err(|e| {
            // The path is safe to name; the contents never are.
            anyhow::anyhow!("reading {} ({}): {e}", ENV_TOKEN_FILE, path.display())
        })?;
        let token = raw.trim().to_string();
        if token.is_empty() {
            anyhow::bail!("{} at {} is empty", ENV_TOKEN_FILE, path.display());
        }
        Ok(token)
    }

    /// Borrow the token to hand to the client. The only accessor, on purpose.
    pub fn token(&self) -> Option<String> {
        self.token.clone()
    }

    pub fn has_token(&self) -> bool {
        self.token.is_some()
    }

    pub fn client(&self) -> super::client::HypemanClient {
        super::client::HypemanClient::new(self.base_url.clone(), self.token())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_never_reveals_the_token() {
        let cfg = Config::new("http://127.0.0.1:4973", Some("super-secret-value".into()));
        let rendered = format!("{cfg:?}");
        assert!(
            !rendered.contains("super-secret-value"),
            "token leaked into Debug: {rendered}"
        );
        assert!(rendered.contains("<redacted>"));
        assert!(rendered.contains("127.0.0.1:4973"), "url should be visible");
    }

    #[test]
    fn debug_distinguishes_absent_from_present() {
        let none = format!("{:?}", Config::new("http://x", None));
        assert!(none.contains("<none>"), "{none}");
    }

    #[test]
    fn blank_tokens_are_treated_as_absent() {
        assert!(!Config::new("http://x", Some("   ".into())).has_token());
        assert!(!Config::new("http://x", Some(String::new())).has_token());
        assert!(Config::new("http://x", Some("t".into())).has_token());
    }

    #[test]
    fn token_file_is_read_and_trimmed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("token");
        std::fs::write(&path, "  tok-from-file\n").unwrap();
        assert_eq!(
            Config::read_token_file(&path).unwrap(),
            "tok-from-file",
            "trailing newline from `echo` must not become part of the secret"
        );
    }

    #[test]
    fn empty_token_file_is_an_error_not_a_silent_no_auth() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("token");
        std::fs::write(&path, "\n").unwrap();
        let err = Config::read_token_file(&path).unwrap_err().to_string();
        assert!(err.contains("is empty"), "{err}");
    }

    #[test]
    fn missing_token_file_names_the_path_not_the_contents() {
        let err = Config::read_token_file(Path::new("/definitely/not/here"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("/definitely/not/here"), "{err}");
    }
}
