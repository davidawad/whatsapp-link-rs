//! The crate's typed error.

use std::path::PathBuf;

/// Result alias used throughout the crate.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Everything that can go wrong. Variants never carry tokens, message bodies
/// or phone numbers.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The `wuzapi` executable could not be started.
    #[error("could not start wuzapi at {bin}: {source}")]
    Spawn {
        bin: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// Another process is already using the data directory.
    #[error(
        "data directory {dir} is already in use{}; never run two clients on one session",
        holders_suffix(.holders)
    )]
    DataDirBusy { dir: PathBuf, holders: Vec<u32> },
    /// Local I/O failure (data directory, lock file, pipes).
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    /// The wuzapi process exited or closed its pipes.
    #[error("wuzapi is no longer running")]
    Disconnected,
    /// No response arrived in time.
    #[error("timed out waiting for `{method}`")]
    Timeout { method: String },
    /// wuzapi answered with a JSON-RPC error.
    #[error("wuzapi rejected `{method}` ({code}): {message}")]
    Remote {
        method: String,
        code: i64,
        message: String,
    },
    /// Message history is disabled for this wuzapi user (it was created with
    /// a history size of 0).
    #[error("message history is disabled for this wuzapi user")]
    HistoryDisabled,
    /// wuzapi answered, but not in the shape this crate understands.
    #[error("unexpected response to `{method}`: {detail}")]
    Protocol { method: String, detail: String },
    /// A caller-supplied value (phone number, JID, ...) is not usable.
    #[error("invalid input: {0}")]
    InvalidInput(String),
    /// The account is already linked, so there is nothing to pair.
    #[error("the account is already linked")]
    AlreadyLinked,
    /// Pairing was attempted and failed on wuzapi's side.
    #[error("pairing failed: {0}")]
    PairingFailed(String),
}

fn holders_suffix(holders: &[u32]) -> String {
    if holders.is_empty() {
        String::new()
    } else {
        let pids: Vec<String> = holders.iter().map(u32::to_string).collect();
        format!(" (pid {})", pids.join(", "))
    }
}
