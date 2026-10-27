//! Process-level tests: a shell script stands in for the `wuzapi` binary.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use whatsapp_link::{Config, Error, Wuzapi};

/// A stub that records its argv/env, answers the provisioning calls and
/// session.status, and exits when stdin closes (like the real thing).
fn write_stub(dir: &Path) -> PathBuf {
    let record = dir.join("record.txt");
    let script = format!(
        r#"#!/bin/sh
{{ for a in "$@"; do echo "arg:$a"; done; echo "admin:$WUZAPI_ADMIN_TOKEN"; echo "tz:$TZ"; }} > '{record}'
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -E 's/.*"id":([0-9]+).*/\1/')
  case "$line" in
    *admin.users.list*) echo "{{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":[]}}" ;;
    *session.status*) echo "{{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{{\"id\":\"u\",\"connected\":false,\"loggedIn\":false,\"jid\":\"\",\"history\":\"5\"}}}}" ;;
    *) echo "{{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{{}}}}" ;;
  esac
done
echo "exited" >> '{record}'
"#,
        record = record.display()
    );
    let path = dir.join("wuzapi-stub.sh");
    fs::write(&path, script).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn config(bin: &Path, data: &Path) -> Config {
    Config {
        wuzapi_bin: bin.to_path_buf(),
        data_dir: data.to_path_buf(),
        user_token: "tok-user".into(),
        admin_token: "tok-admin".into(),
        timezone: "Europe/Paris".into(),
    }
}

#[test]
fn spawn_passes_flags_and_environment_and_shuts_down_on_drop() {
    let tmp = tempfile::tempdir().unwrap();
    let bin = write_stub(tmp.path());
    let data = tmp.path().join("data");
    let record = tmp.path().join("record.txt");

    let wa = Wuzapi::spawn(config(&bin, &data)).unwrap();
    assert!(!wa.status().unwrap().connected);
    drop(wa);

    let recorded = fs::read_to_string(&record).unwrap();
    let canonical = data.canonicalize().unwrap();
    assert!(recorded.contains("arg:-mode=stdio"), "{recorded}");
    assert!(
        recorded.contains(&format!("arg:-datadir={}", canonical.display())),
        "{recorded}"
    );
    assert!(recorded.contains("admin:tok-admin"));
    assert!(recorded.contains("tz:Europe/Paris"));
    assert!(
        recorded.contains("exited"),
        "child saw EOF on stdin: {recorded}"
    );
}

#[test]
fn a_second_client_on_the_same_data_dir_is_refused_until_the_first_drops() {
    let tmp = tempfile::tempdir().unwrap();
    let bin = write_stub(tmp.path());
    let data = tmp.path().join("data");

    let first = Wuzapi::spawn(config(&bin, &data)).unwrap();
    match Wuzapi::spawn(config(&bin, &data)) {
        Err(Error::DataDirBusy { dir, .. }) => assert_eq!(dir, data.canonicalize().unwrap()),
        other => panic!("expected DataDirBusy, got {other:?}"),
    }
    drop(first);
    Wuzapi::spawn(config(&bin, &data)).unwrap();
}

#[test]
fn a_foreign_process_using_the_data_dir_blocks_startup() {
    let tmp = tempfile::tempdir().unwrap();
    let bin = write_stub(tmp.path());
    let data = tmp.path().join("data");
    fs::create_dir_all(&data).unwrap();
    let canonical = data.canonicalize().unwrap();

    // Looks like `sh -c "sleep 30; :" sh -datadir=<dir>` in the process table.
    let mut foreign = Command::new("sh")
        .args(["-c", "sleep 30; :", "sh"])
        .arg(format!("-datadir={}", canonical.display()))
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    // Give the process table a moment to show the new process.
    let deadline = Instant::now() + Duration::from_secs(5);
    let result = loop {
        let r = Wuzapi::spawn(config(&bin, &data));
        if r.is_err() || Instant::now() > deadline {
            break r;
        }
        drop(r);
        std::thread::sleep(Duration::from_millis(50));
    };
    let _ = foreign.kill();
    let _ = foreign.wait();
    match result {
        Err(Error::DataDirBusy { holders, .. }) => assert!(holders.contains(&foreign.id())),
        other => panic!("expected DataDirBusy, got {other:?}"),
    }
    // Once the foreign process is gone the directory is free again.
    Wuzapi::spawn(config(&bin, &data)).unwrap();
}

#[test]
fn a_missing_binary_is_a_typed_spawn_error() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = config(&tmp.path().join("nope"), &tmp.path().join("data"));
    assert!(matches!(Wuzapi::spawn(cfg), Err(Error::Spawn { .. })));
}

fn cli(tmp: &Path, bin: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_whatsapp-link"));
    cmd.env_remove("WHATSAPP_LINK_DATA_DIR")
        .env_remove("WHATSAPP_LINK_USER_TOKEN")
        .arg("--wuzapi-bin")
        .arg(bin)
        .arg("--data-dir")
        .arg(tmp.join("data"));
    cmd
}

#[test]
fn cli_status_prints_json() {
    let tmp = tempfile::tempdir().unwrap();
    let bin = write_stub(tmp.path());
    let out = cli(tmp.path(), &bin).arg("status").output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["state"], "needs_pairing");
    assert_eq!(v["history"], 5);
}

#[test]
fn cli_reports_errors_as_json_on_stderr() {
    let tmp = tempfile::tempdir().unwrap();
    let out = cli(tmp.path(), &tmp.path().join("missing"))
        .arg("status")
        .output()
        .unwrap();
    assert!(!out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stderr).unwrap();
    assert!(v["error"]
        .as_str()
        .unwrap()
        .contains("could not start wuzapi"));
}

#[test]
fn cli_rejects_bad_arguments_before_starting_anything() {
    let tmp = tempfile::tempdir().unwrap();
    let bin = write_stub(tmp.path());
    for args in [&["send", "12", "hi"][..], &["history", "nojid"], &["link"]] {
        let out = cli(tmp.path(), &bin).args(args).output().unwrap();
        assert!(!out.status.success(), "{args:?}");
        assert!(
            !tmp.path().join("record.txt").exists(),
            "stub must not have run: {args:?}"
        );
    }
}
