//! Configuration: where the `wuzapi` binary is, where its data lives and which
//! tokens identify the session.

use std::fmt;
use std::path::PathBuf;

/// Environment variable names understood by [`Config::from_env`].
pub mod env {
    pub const WUZAPI_BIN: &str = "WHATSAPP_LINK_WUZAPI_BIN";
    pub const DATA_DIR: &str = "WHATSAPP_LINK_DATA_DIR";
    pub const USER_TOKEN: &str = "WHATSAPP_LINK_USER_TOKEN";
    pub const ADMIN_TOKEN: &str = "WHATSAPP_LINK_ADMIN_TOKEN";
    pub const TIMEZONE: &str = "WHATSAPP_LINK_TIMEZONE";
}

/// Everything needed to start a `wuzapi` child and address one session in it.
///
/// Any existing wuzapi data directory plus the user token it was created with
/// can be reused: point `data_dir` and `user_token` at them.
#[derive(Clone, PartialEq, Eq)]
pub struct Config {
    /// The `wuzapi` executable. Defaults to `wuzapi`, resolved through `PATH`.
    pub wuzapi_bin: PathBuf,
    /// wuzapi's `-datadir` (its database and session files).
    pub data_dir: PathBuf,
    /// Token of the wuzapi user that owns the WhatsApp session.
    pub user_token: String,
    /// Token for wuzapi's admin calls (used only to create the user).
    pub admin_token: String,
    /// IANA timezone name passed to the child as `TZ`.
    pub timezone: String,
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("wuzapi_bin", &self.wuzapi_bin)
            .field("data_dir", &self.data_dir)
            .field("user_token", &"<redacted>")
            .field("admin_token", &"<redacted>")
            .field("timezone", &self.timezone)
            .finish()
    }
}

impl Default for Config {
    fn default() -> Self {
        Self::resolve(|key| std::env::var(key).ok())
    }
}

impl Config {
    /// Defaults overridden by the `WHATSAPP_LINK_*` environment variables.
    pub fn from_env() -> Self {
        Self::default()
    }

    /// Pure resolution against an arbitrary environment lookup (testable).
    pub fn resolve(get: impl Fn(&str) -> Option<String>) -> Self {
        let non_empty = |key: &str| get(key).filter(|v| !v.is_empty());
        let login = login_name(&get);
        Config {
            wuzapi_bin: non_empty(env::WUZAPI_BIN)
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("wuzapi")),
            data_dir: non_empty(env::DATA_DIR)
                .map(PathBuf::from)
                .unwrap_or_else(|| default_data_dir(&get)),
            user_token: non_empty(env::USER_TOKEN).unwrap_or_else(|| derive_token("user", &login)),
            admin_token: non_empty(env::ADMIN_TOKEN)
                .unwrap_or_else(|| derive_token("admin", &login)),
            timezone: non_empty(env::TIMEZONE)
                .or_else(|| non_empty("TZ"))
                .unwrap_or_else(|| "UTC".to_string()),
        }
    }
}

fn login_name(get: &impl Fn(&str) -> Option<String>) -> String {
    ["USER", "LOGNAME", "USERNAME"]
        .iter()
        .find_map(|k| get(k).filter(|v| !v.is_empty()))
        .unwrap_or_else(|| "user".to_string())
}

fn default_data_dir(get: &impl Fn(&str) -> Option<String>) -> PathBuf {
    let non_empty = |key: &str| get(key).filter(|v| !v.is_empty());
    non_empty("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| non_empty("HOME").map(|h| PathBuf::from(h).join(".local").join("share")))
        .or_else(|| non_empty("LOCALAPPDATA").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("whatsapp-link")
}

/// Stable token derived from a purpose and the login name (FNV-1a, so it never
/// changes between releases of this crate or of Rust). wuzapi only listens on
/// this process's own stdio, so the token identifies the session; it is not a
/// network credential.
pub fn derive_token(purpose: &str, login: &str) -> String {
    let fnv = |seed: u64| {
        format!("whatsapp-link:{purpose}:{login}")
            .bytes()
            .fold(0xcbf2_9ce4_8422_2325 ^ seed, |h, b| {
                (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
            })
    };
    format!("wl-{purpose}-{:016x}{:016x}", fnv(0), fnv(0x9e37_79b9))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k| map.get(k).cloned()
    }

    #[test]
    fn defaults_follow_xdg_and_login() {
        let c = Config::resolve(env_of(&[("HOME", "/home/alice"), ("USER", "alice")]));
        assert_eq!(c.wuzapi_bin, PathBuf::from("wuzapi"));
        assert_eq!(
            c.data_dir,
            PathBuf::from("/home/alice/.local/share/whatsapp-link")
        );
        assert_eq!(c.user_token, derive_token("user", "alice"));
        assert_ne!(c.user_token, c.admin_token);
        assert_eq!(c.timezone, "UTC");
    }

    #[test]
    fn xdg_data_home_wins_over_home() {
        let c = Config::resolve(env_of(&[("HOME", "/h"), ("XDG_DATA_HOME", "/x")]));
        assert_eq!(c.data_dir, PathBuf::from("/x/whatsapp-link"));
    }

    #[test]
    fn env_overrides_everything() {
        let c = Config::resolve(env_of(&[
            (env::WUZAPI_BIN, "/opt/wuzapi"),
            (env::DATA_DIR, "/data"),
            (env::USER_TOKEN, "u"),
            (env::ADMIN_TOKEN, "a"),
            (env::TIMEZONE, "Europe/Paris"),
        ]));
        assert_eq!(c.wuzapi_bin, PathBuf::from("/opt/wuzapi"));
        assert_eq!(c.data_dir, PathBuf::from("/data"));
        assert_eq!((c.user_token.as_str(), c.admin_token.as_str()), ("u", "a"));
        assert_eq!(c.timezone, "Europe/Paris");
    }

    #[test]
    fn derived_tokens_are_stable_and_distinct_per_login() {
        assert_eq!(derive_token("user", "a"), derive_token("user", "a"));
        assert_ne!(derive_token("user", "a"), derive_token("user", "b"));
    }

    #[test]
    fn debug_redacts_tokens() {
        let c = Config::resolve(env_of(&[(env::USER_TOKEN, "super-secret")]));
        assert!(!format!("{c:?}").contains("super-secret"));
    }
}
