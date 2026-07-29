#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

use assert_cmd::cargo::CommandCargoExt;
use tempfile::TempDir;

#[test]
fn status_observes_the_default_profile_and_prints_every_reported_quota_window() {
    let fixture = FakeCodex::new(
        r#"{
  "rateLimits": {
    "limitId": "legacy",
    "limitName": "Legacy",
    "primary": {
      "usedPercent": 99,
      "windowDurationMins": 5,
      "resetsAt": 1730947200
    },
    "secondary": null
  },
  "rateLimitsByLimitId": {
    "codex": {
      "limitId": "codex",
      "limitName": "Codex",
      "primary": {
        "usedPercent": 25,
        "windowDurationMins": 15,
        "resetsAt": 1730947200
      },
      "secondary": {
        "usedPercent": 60,
        "windowDurationMins": 10080,
        "resetsAt": 1731552000
      }
    },
    "codex_other": {
      "limitId": "codex_other",
      "limitName": null,
      "primary": {
        "usedPercent": 42.5,
        "windowDurationMins": 60,
        "resetsAt": 1730950800
      },
      "secondary": null
    }
  }
}"#,
    );
    let codex_home = fixture.root.path().join("profile-home");
    fs::create_dir(&codex_home).expect("create fake Codex Home");

    let output = fixture
        .command(&codex_home)
        .arg("status")
        .output()
        .expect("run limitr");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).expect("UTF-8 output"),
        "\
Profile: default
Identity: developer@example.com
Plan: plus

Limit Bucket: Codex (codex)
  Primary: 25% used; 15 min window; resets 2024-11-07T02:40:00Z
  Secondary: 60% used; 10080 min window; resets 2024-11-14T02:40:00Z

Limit Bucket: codex_other
  Primary: 42.5% used; 60 min window; resets 2024-11-07T03:40:00Z
"
    );
}

#[test]
fn status_falls_back_to_the_legacy_bucket_when_no_multi_bucket_limits_are_reported() {
    let fixture = FakeCodex::new(
        r#"{
  "rateLimits": {
    "limitId": "codex",
    "limitName": "Legacy Codex",
    "primary": {
      "usedPercent": 73,
      "windowDurationMins": 300,
      "resetsAt": 1730950800
    },
    "secondary": null
  },
  "rateLimitsByLimitId": {}
}"#,
    );
    let codex_home = fixture.root.path().join("profile-home");
    fs::create_dir(&codex_home).expect("create fake Codex Home");

    let output = fixture
        .command(&codex_home)
        .arg("status")
        .output()
        .expect("run limitr");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).expect("UTF-8 output"),
        "\
Profile: default
Identity: developer@example.com
Plan: plus

Limit Bucket: Legacy Codex (codex)
  Primary: 73% used; 300 min window; resets 2024-11-07T03:40:00Z
"
    );
}

#[test]
fn status_synthesizes_the_normal_codex_home_when_codex_home_is_not_set() {
    let fixture = FakeCodex::new(
        r#"{
  "rateLimits": {
    "limitId": "codex",
    "limitName": null,
    "primary": {
      "usedPercent": 10,
      "windowDurationMins": 60,
      "resetsAt": 1730950800
    },
    "secondary": null
  },
  "rateLimitsByLimitId": null
}"#,
    );
    let home = fixture.root.path().join("person-home");
    let expected_codex_home = home.join(".codex");
    fs::create_dir_all(&expected_codex_home).expect("create normal Codex Home");
    let mut command = fixture.command(&expected_codex_home);
    command.env_remove("CODEX_HOME").env("HOME", home);

    let output = command.arg("status").output().expect("run limitr");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).expect("UTF-8 output"),
        "\
Profile: default
Identity: developer@example.com
Plan: plus

Limit Bucket: codex
  Primary: 10% used; 60 min window; resets 2024-11-07T03:40:00Z
"
    );
}

struct FakeCodex {
    root: TempDir,
}

impl FakeCodex {
    fn new(rate_limits_result: &str) -> Self {
        let root = tempfile::tempdir().expect("create fixture directory");
        let executable = root.path().join("codex");
        let rate_limits_result = serde_json::to_string(
            &serde_json::from_str::<serde_json::Value>(rate_limits_result)
                .expect("valid fake rate-limit result"),
        )
        .expect("serialize fake rate-limit result");
        let script = format!(
            r#"#!/bin/sh
set -eu

[ "$#" -eq 2 ]
[ "$1" = "app-server" ]
[ "$2" = "--stdio" ]
[ "$CODEX_HOME" = "$FAKE_EXPECTED_CODEX_HOME" ]

read -r message
case "$message" in
  *'"method":"initialize"'*) ;;
  *) exit 90 ;;
esac
printf '%s\n' '{{"id":0,"result":{{"userAgent":"fake","platformFamily":"unix","platformOs":"test"}}}}'

read -r message
case "$message" in
  *'"method":"initialized"'*) ;;
  *) exit 91 ;;
esac

read -r message
case "$message" in
  *'"method":"account/read"'*'"refreshToken":false'*) ;;
  *) exit 92 ;;
esac
printf '%s\n' '{{"id":1,"result":{{"account":{{"type":"chatgpt","email":"developer@example.com","planType":"plus"}},"requiresOpenaiAuth":true}}}}'

read -r message
case "$message" in
  *'"method":"account/rateLimits/read"'*) ;;
  *) exit 93 ;;
esac
printf '%s\n' '{{"id":2,"result":{rate_limits_result}}}'
"#
        );
        fs::write(&executable, script).expect("write fake codex");
        let mut permissions = fs::metadata(&executable)
            .expect("read fake codex metadata")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&executable, permissions).expect("make fake codex executable");

        Self { root }
    }

    fn command(&self, codex_home: &Path) -> Command {
        let mut command = Command::cargo_bin("limitr").expect("locate limitr binary");
        let path = std::env::join_paths(std::iter::once(self.root.path().to_path_buf()).chain(
            std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
        ))
        .expect("construct PATH");
        command
            .env("PATH", path)
            .env("CODEX_HOME", codex_home)
            .env("FAKE_EXPECTED_CODEX_HOME", codex_home);
        command
    }
}
