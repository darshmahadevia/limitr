#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Output};

use assert_cmd::cargo::CommandCargoExt;
use tempfile::TempDir;

#[test]
fn status_reports_an_actionable_unauthenticated_profile_state() {
    let fixture = ResilienceFixture::new();

    let output = fixture.status("unauthenticated");

    assert_profile_failure(
        &output,
        "Account Profile: default\nUnauthenticated: sign in to this Codex Home with Codex, then retry\n",
    );
}

#[test]
fn status_reports_api_key_auth_as_unsupported_for_chatgpt_limits() {
    let fixture = ResilienceFixture::new();

    let output = fixture.status("api-key");

    assert_profile_failure(
        &output,
        "Account Profile: default\nUnsupported: API-key authentication does not provide ChatGPT rate limits\n",
    );
}

#[test]
fn status_reports_bedrock_auth_as_unsupported_for_chatgpt_limits() {
    let fixture = ResilienceFixture::new();

    let output = fixture.status("bedrock");

    assert_profile_failure(
        &output,
        "Account Profile: default\nUnsupported: Bedrock authentication does not provide ChatGPT rate limits\n",
    );
}

#[test]
fn status_scopes_a_missing_required_method_to_the_profile() {
    let fixture = ResilienceFixture::new();

    let output = fixture.status("missing-method");

    assert_profile_failure(
        &output,
        "Account Profile: default\nError: incompatible Codex app-server: required method `account/rateLimits/read` is unavailable; update Codex\n",
    );
}

#[test]
fn status_names_a_missing_required_field_without_echoing_response_data() {
    let fixture = ResilienceFixture::new();

    let output = fixture.status("missing-field");

    assert_profile_failure(
        &output,
        "Account Profile: default\nError: incompatible Codex app-server response to `account/rateLimits/read`: missing required field `rateLimits`; update Codex\n",
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains("private@example.com"));
}

#[test]
fn status_names_a_missing_required_quota_window_field() {
    let fixture = ResilienceFixture::new();

    let output = fixture.status("missing-window-field");

    assert_profile_failure(
        &output,
        "Account Profile: default\nError: incompatible Codex app-server response to `account/rateLimits/read`: missing required field `rateLimits.primary.resetsAt`; update Codex\n",
    );
}

#[test]
fn status_accepts_chatgpt_identity_metadata_when_it_is_unavailable() {
    let fixture = ResilienceFixture::new();

    let output = fixture.status("optional-identity");

    assert!(output.status.success());
    assert_eq!(output.stderr, b"");
    let stdout = String::from_utf8(output.stdout).expect("UTF-8 output");
    assert!(stdout.contains("Account Profile: default"));
    assert!(stdout.contains("25% used"));
    assert!(!stdout.contains("Account Identity:"));
    assert!(!stdout.contains("Plan:"));
}

#[test]
fn status_distinguishes_an_absent_account_field_from_unauthenticated() {
    let fixture = ResilienceFixture::new();

    let output = fixture.status("missing-account");

    assert_profile_failure(
        &output,
        "Account Profile: default\nError: incompatible Codex app-server response to `account/read`: missing required field `account`; update Codex\n",
    );
}

#[test]
fn status_classifies_a_missing_json_rpc_result_as_incompatible() {
    let fixture = ResilienceFixture::new();

    let output = fixture.status("missing-result");

    assert_profile_failure(
        &output,
        "Account Profile: default\nError: incompatible Codex app-server response to `account/rateLimits/read`: missing required `result`; update Codex\n",
    );
}

#[test]
fn status_names_the_limit_id_for_an_incompatible_multi_bucket_window() {
    let fixture = ResilienceFixture::new();

    let output = fixture.status("missing-multi-bucket-field");

    assert_profile_failure(
        &output,
        "Account Profile: default\nError: incompatible Codex app-server response to `account/rateLimits/read`: missing required field `rateLimitsByLimitId.codex_other.primary.resetsAt`; update Codex\n",
    );
}

#[test]
fn compatibility_diagnostics_redact_a_credential_shaped_limit_id() {
    let fixture = ResilienceFixture::new();

    let output = fixture.status("sensitive-bucket-id");

    assert_eq!(output.status.code(), Some(2));
    let stdout = String::from_utf8(output.stdout).expect("UTF-8 output");
    assert!(stdout.contains("rateLimitsByLimitId.<redacted>.primary.resetsAt"));
    assert!(!stdout.contains("sk-secret"));
}

fn assert_profile_failure(output: &Output, expected_stdout: &str) {
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(output.stdout, expected_stdout.as_bytes());
    assert_eq!(output.stderr, b"");
}

struct ResilienceFixture {
    root: TempDir,
}

impl ResilienceFixture {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("create fixture directory");
        let executable = root.path().join("codex");
        fs::write(
            &executable,
            r#"#!/bin/sh
set -eu

read -r message
printf '%s\n' '{"id":0,"result":{"userAgent":"fake","platformFamily":"unix","platformOs":"test"}}'
read -r message
read -r message
case "$FAKE_MODE" in
  unauthenticated)
    printf '%s\n' '{"id":1,"result":{"account":null,"requiresOpenaiAuth":true}}'
    ;;
  api-key)
    printf '%s\n' '{"id":1,"result":{"account":{"type":"apiKey"},"requiresOpenaiAuth":true}}'
    ;;
  bedrock)
    printf '%s\n' '{"id":1,"result":{"account":{"type":"amazonBedrock"},"requiresOpenaiAuth":true}}'
    ;;
  missing-method|missing-field|missing-window-field)
    printf '%s\n' '{"id":1,"result":{"account":{"type":"chatgpt","email":"private@example.com","planType":"plus"},"requiresOpenaiAuth":true}}'
    read -r message
    if [ "$FAKE_MODE" = "missing-method" ]; then
      printf '%s\n' '{"id":2,"error":{"code":-32601,"message":"account/rateLimits/read missing for private@example.com token sk-secret"}}'
    elif [ "$FAKE_MODE" = "missing-field" ]; then
      printf '%s\n' '{"id":2,"result":{"rateLimitsByLimitId":null,"diagnostic":"private@example.com token sk-secret"}}'
    else
      printf '%s\n' '{"id":2,"result":{"rateLimits":{"limitId":"codex","primary":{"usedPercent":25,"windowDurationMins":60},"secondary":null},"rateLimitsByLimitId":null}}'
    fi
    ;;
  missing-account)
    printf '%s\n' '{"id":1,"result":{"requiresOpenaiAuth":true}}'
    ;;
  optional-identity|missing-result|missing-multi-bucket-field|sensitive-bucket-id)
    printf '%s\n' '{"id":1,"result":{"account":{"type":"chatgpt"},"requiresOpenaiAuth":true}}'
    read -r message
    case "$FAKE_MODE" in
      optional-identity)
        printf '%s\n' '{"id":2,"result":{"rateLimits":{"limitId":"codex","primary":{"usedPercent":25,"windowDurationMins":60,"resetsAt":4102444800},"secondary":null},"rateLimitsByLimitId":null}}'
        ;;
      missing-result)
        printf '%s\n' '{"id":2}'
        ;;
      missing-multi-bucket-field)
        printf '%s\n' '{"id":2,"result":{"rateLimits":{"limitId":"codex","primary":{"usedPercent":25,"windowDurationMins":60,"resetsAt":4102444800},"secondary":null},"rateLimitsByLimitId":{"codex_other":{"primary":{"usedPercent":50,"windowDurationMins":60},"secondary":null}}}}'
        ;;
      sensitive-bucket-id)
        printf '%s\n' '{"id":2,"result":{"rateLimits":{"limitId":"codex","primary":{"usedPercent":25,"windowDurationMins":60,"resetsAt":4102444800},"secondary":null},"rateLimitsByLimitId":{"sk-secret":{"primary":{"usedPercent":50,"windowDurationMins":60},"secondary":null}}}}'
        ;;
    esac
    ;;
  *)
    exit 89
    ;;
esac

# Unsupported authentication must be decided from account/read.
if read -r unexpected; then
  exit 88
fi
"#,
        )
        .expect("write fake codex");
        let mut permissions = fs::metadata(&executable)
            .expect("read fake codex metadata")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&executable, permissions).expect("make fake codex executable");
        Self { root }
    }

    fn status(&self, mode: &str) -> Output {
        let mut command = Command::cargo_bin("limitr").expect("locate limitr binary");
        let path = std::env::join_paths(std::iter::once(self.root.path().to_path_buf()).chain(
            std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
        ))
        .expect("construct PATH");
        command
            .env("PATH", path)
            .env("XDG_CONFIG_HOME", self.root.path().join("config"))
            .env("CODEX_HOME", self.root.path().join("codex-home"))
            .env("FAKE_MODE", mode)
            .arg("status")
            .output()
            .expect("run limitr")
    }
}
