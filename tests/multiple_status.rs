#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Output};

use assert_cmd::cargo::CommandCargoExt;
use tempfile::TempDir;

#[test]
fn status_observes_configured_profiles_concurrently_and_prints_configured_order() {
    let fixture = MultipleStatusFixture::new();
    let first_home = fixture.codex_home("first");
    let second_home = fixture.codex_home("second");
    fixture.success(&[
        "profile",
        "add",
        "personal",
        first_home.to_str().expect("UTF-8 fixture path"),
    ]);
    fixture.success(&[
        "profile",
        "add",
        "work",
        second_home.to_str().expect("UTF-8 fixture path"),
    ]);

    let output = fixture.success(&["status"]);

    assert_eq!(
        String::from_utf8(output.stdout).expect("UTF-8 output"),
        "\
Account Profile: personal
Account Identity: personal@example.com
Plan: plus

Limit Bucket: codex
  Primary: 20% used; 60 min window; resets 2024-11-07T03:40:00Z

Account Profile: work
Account Identity: work@example.com
Plan: team

Limit Bucket: codex
  Primary: 80% used; 60 min window; resets 2024-11-07T03:40:00Z
"
    );
}

#[test]
fn status_flags_duplicate_account_identities_without_collapsing_profiles() {
    let fixture = MultipleStatusFixture::new();
    let first_home = fixture.codex_home("first");
    let second_home = fixture.codex_home("second");
    fixture.success(&[
        "profile",
        "add",
        "personal",
        first_home.to_str().expect("UTF-8 fixture path"),
    ]);
    fixture.success(&[
        "profile",
        "add",
        "work",
        second_home.to_str().expect("UTF-8 fixture path"),
    ]);

    let output = fixture
        .command()
        .env("FAKE_DUPLICATE_IDENTITY", "1")
        .arg("status")
        .output()
        .expect("run limitr");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("UTF-8 output");
    assert_eq!(stdout.matches("Account Profile: ").count(), 2);
    assert_eq!(
        stdout
            .matches("Account Identity: shared@example.com")
            .count(),
        2
    );
    assert_eq!(
        stdout
            .matches("Duplicate Account Identity: Account Profiles personal, work")
            .count(),
        2
    );
}

#[test]
fn status_isolates_a_failed_profile_and_returns_partial_failure_exit_status() {
    let fixture = MultipleStatusFixture::new();
    let first_home = fixture.codex_home("first");
    let second_home = fixture.codex_home("second");
    fixture.success(&[
        "profile",
        "add",
        "personal",
        first_home.to_str().expect("UTF-8 fixture path"),
    ]);
    fixture.success(&[
        "profile",
        "add",
        "work",
        second_home.to_str().expect("UTF-8 fixture path"),
    ]);

    let output = fixture
        .command()
        .env("FAKE_FAIL_PROFILE", "second")
        .arg("status")
        .output()
        .expect("run limitr");

    assert_eq!(output.status.code(), Some(2));
    assert_eq!(output.stderr, b"");
    assert_eq!(
        String::from_utf8(output.stdout).expect("UTF-8 output"),
        "\
Account Profile: personal
Account Identity: personal@example.com
Plan: plus

Limit Bucket: codex
  Primary: 20% used; 60 min window; resets 2024-11-07T03:40:00Z

Account Profile: work
Error: Codex app-server request `account/rateLimits/read` failed; check the Account Profile authentication and Codex compatibility
"
    );
}

struct MultipleStatusFixture {
    root: TempDir,
}

impl MultipleStatusFixture {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("create fixture directory");
        let executable = root.path().join("codex");
        fs::write(
            &executable,
            r#"#!/bin/sh
set -eu

[ "$#" -eq 2 ]
[ "$1" = "app-server" ]
[ "$2" = "--stdio" ]

profile="$(basename "$CODEX_HOME")"
case "$profile" in
  first)
    email="personal@example.com"
    plan="plus"
    used="20"
    ;;
  second)
    email="work@example.com"
    plan="team"
    used="80"
    ;;
  *)
    exit 89
    ;;
esac
if [ "${FAKE_DUPLICATE_IDENTITY:-}" = "1" ]; then
  email="shared@example.com"
  plan="plus"
fi

touch "$FAKE_MARKER_DIR/$profile"
attempt=0
while [ ! -f "$FAKE_MARKER_DIR/first" ] || [ ! -f "$FAKE_MARKER_DIR/second" ]; do
  attempt=$((attempt + 1))
  [ "$attempt" -lt 200 ] || exit 88
  sleep 0.01
done

read -r message
case "$message" in
  *'"method":"initialize"'*) ;;
  *) exit 90 ;;
esac
printf '%s\n' '{"id":0,"result":{"userAgent":"fake","platformFamily":"unix","platformOs":"test"}}'

read -r message
case "$message" in
  *'"method":"initialized"'*) ;;
  *) exit 91 ;;
esac

read -r message
case "$message" in
  *'"method":"account/read"'*) ;;
  *) exit 92 ;;
esac
printf '{"id":1,"result":{"account":{"type":"chatgpt","email":"%s","planType":"%s"}}}\n' "$email" "$plan"

read -r message
case "$message" in
  *'"method":"account/rateLimits/read"'*) ;;
  *) exit 93 ;;
esac
if [ "${FAKE_FAIL_PROFILE:-}" = "$profile" ]; then
  printf '%s\n' '{"id":2,"error":{"code":-32000,"message":"token sk-sensitive belongs to work@example.com"}}'
  exit 0
fi
[ "$profile" != "first" ] || sleep 0.1
printf '{"id":2,"result":{"rateLimits":{"limitId":"codex","limitName":null,"primary":{"usedPercent":%s,"windowDurationMins":60,"resetsAt":1730950800},"secondary":null},"rateLimitsByLimitId":null}}\n' "$used"
"#,
        )
        .expect("write fake codex");
        let mut permissions = fs::metadata(&executable)
            .expect("read fake codex metadata")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&executable, permissions).expect("make fake codex executable");
        fs::create_dir(root.path().join("markers")).expect("create marker directory");
        Self { root }
    }

    fn codex_home(&self, name: &str) -> std::path::PathBuf {
        let path = self.root.path().join(name);
        fs::create_dir(&path).expect("create fake Codex Home");
        path
    }

    fn command(&self) -> Command {
        let mut command = Command::cargo_bin("limitr").expect("locate limitr binary");
        let path = std::env::join_paths(std::iter::once(self.root.path().to_path_buf()).chain(
            std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
        ))
        .expect("construct PATH");
        command
            .env("PATH", path)
            .env("XDG_CONFIG_HOME", self.root.path().join("config"))
            .env("FAKE_MARKER_DIR", self.root.path().join("markers"));
        command
    }

    fn success(&self, arguments: &[&str]) -> Output {
        let output = self.command().args(arguments).output().expect("run limitr");
        assert!(
            output.status.success(),
            "stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }
}
