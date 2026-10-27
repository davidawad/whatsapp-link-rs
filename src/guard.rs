//! Concurrency guard: never let two clients drive one wuzapi data directory.
//!
//! Two independent checks:
//! 1. an advisory lock on `<data_dir>/.whatsapp-link.lock`, held for the life
//!    of the guard (stops two instances of this crate), and
//! 2. a process-table scan for any other process started with this directory
//!    as its `-datadir` (stops a foreign wuzapi, or another client, from
//!    sharing the session).

use std::fs::{self, File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::{Error, Result};

const LOCK_FILE: &str = ".whatsapp-link.lock";

/// Held for as long as a client owns the data directory.
#[derive(Debug)]
pub struct DataDirGuard {
    _lock: File,
    dir: PathBuf,
}

impl DataDirGuard {
    /// Create the directory if needed and claim it, or fail with
    /// [`Error::DataDirBusy`].
    pub fn acquire(dir: &Path) -> Result<Self> {
        fs::create_dir_all(dir)?;
        let canonical = dir.canonicalize()?;

        let mut holders = foreign_holders(&canonical, dir, std::process::id());
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(canonical.join(LOCK_FILE))?;
        match lock.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                return Err(Error::DataDirBusy {
                    dir: canonical,
                    holders,
                })
            }
            Err(TryLockError::Error(e)) => return Err(Error::Io(e)),
        }
        if !holders.is_empty() {
            holders.sort_unstable();
            return Err(Error::DataDirBusy {
                dir: canonical,
                holders,
            });
        }
        Ok(DataDirGuard {
            _lock: lock,
            dir: canonical,
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

fn foreign_holders(canonical: &Path, given: &Path, own_pid: u32) -> Vec<u32> {
    let listing = Command::new("ps")
        .args(["-axo", "pid=,command="])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    let dirs = [
        canonical.to_string_lossy().into_owned(),
        given.to_string_lossy().into_owned(),
    ];
    find_holders(&listing, &dirs, own_pid)
}

/// Pure core: PIDs in a `ps -axo pid=,command=` listing whose command line
/// passes any of `dirs` as `-datadir`. Handles `-datadir=DIR`, `-datadir DIR`
/// and `--datadir…`, and paths that contain spaces.
pub(crate) fn find_holders(ps_listing: &str, dirs: &[String], own_pid: u32) -> Vec<u32> {
    let dirs: Vec<&str> = dirs
        .iter()
        .map(|d| d.trim_end_matches('/'))
        .filter(|d| !d.is_empty())
        .collect();
    ps_listing
        .lines()
        .filter_map(|line| {
            let line = line.trim_start();
            let (pid, command) = line.split_once(char::is_whitespace)?;
            let pid: u32 = pid.parse().ok()?;
            (pid != own_pid && dirs.iter().any(|d| mentions_datadir(command, d))).then_some(pid)
        })
        .collect()
}

fn mentions_datadir(command: &str, dir: &str) -> bool {
    ["-datadir=", "--datadir=", "-datadir ", "--datadir "]
        .iter()
        .any(|flag| {
            command.match_indices(flag).any(|(at, _)| {
                let after = command[at + flag.len()..].trim_start_matches('"');
                after.strip_prefix(dir).is_some_and(|rest| {
                    let rest = rest.trim_start_matches('/');
                    rest.is_empty() || rest.starts_with([' ', '"'])
                })
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PS: &str = "\
  101 /sbin/launchd
  202 /opt/bin/wuzapi -mode=stdio -datadir=/data/wa
  303 /opt/bin/wuzapi -mode=stdio -datadir=/data/wa-other
  404 /opt/bin/wuzapi -mode=stdio -datadir /data/wa -osname x
  505 wuzapi -datadir=/data/wa/
  606 /usr/bin/tool --datadir=/data/with space -x
  707 grep -datadir=/data/wa-nothing
";

    fn holders(dir: &str, own: u32) -> Vec<u32> {
        find_holders(PS, &[dir.to_string()], own)
    }

    #[test]
    fn matches_exact_directories_only() {
        assert_eq!(holders("/data/wa", 1), vec![202, 404, 505]);
        assert_eq!(holders("/data/wa-other", 1), vec![303]);
        assert_eq!(holders("/data/wa/", 1), vec![202, 404, 505]);
    }

    #[test]
    fn handles_spaces_and_ignores_own_pid() {
        assert_eq!(holders("/data/with space", 1), vec![606]);
        assert_eq!(holders("/data/wa", 202), vec![404, 505]);
        assert!(holders("/nowhere", 1).is_empty());
    }

    #[test]
    fn second_guard_on_same_dir_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let first = DataDirGuard::acquire(tmp.path()).unwrap();
        match DataDirGuard::acquire(tmp.path()) {
            Err(Error::DataDirBusy { .. }) => {}
            other => panic!("expected DataDirBusy, got {other:?}"),
        }
        drop(first);
        DataDirGuard::acquire(tmp.path()).unwrap();
    }
}
