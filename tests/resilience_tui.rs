#![cfg(unix)]

use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use assert_cmd::cargo::cargo_bin;
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use tempfile::TempDir;

const BEHAVIOR_RESPONSE_TIMEOUT_MS: u64 = 5_000;
const HANG_RESPONSE_TIMEOUT_MS: u64 = 100;

#[test]
fn a_disconnected_profile_keeps_a_stale_snapshot_then_recovers() {
    let fixture = TuiResilienceFixture::new();
    let mut session = fixture.spawn("recover", BEHAVIOR_RESPONSE_TIMEOUT_MS);

    fixture.wait_for("first-snapshot");
    fs::write(fixture.state().join("allow-disconnect"), "").expect("allow disconnect");
    fixture.wait_for("retry-started");
    session.wait_until_screen_contains("Stale Snapshot: observed");
    assert!(session.contents().contains("25% used"));

    fs::write(fixture.state().join("allow-recovery"), "").expect("allow recovery");
    session.wait_until(|| {
        let contents = session.contents();
        contents.contains("75% used") && !contents.contains("Stale Snapshot:")
    });

    session.quit_and_assert_clean();
}

#[test]
fn a_hanging_child_retries_with_bounded_backoff_and_quits_responsively() {
    let fixture = TuiResilienceFixture::new();
    let mut session = fixture.spawn("hang", HANG_RESPONSE_TIMEOUT_MS);

    fixture.wait_until(|| fixture.attempt_count() >= 3);
    thread::sleep(Duration::from_millis(500));
    assert!(
        fixture.attempt_count() <= 6,
        "retry count was not bounded: {}",
        fixture.attempt_count()
    );

    let quit_at = Instant::now();
    session.quit_and_assert_clean();
    assert!(quit_at.elapsed() < Duration::from_secs(2));
    fixture.assert_recorded_children_reaped();
}

#[test]
fn a_sparse_notification_reconciles_without_erasing_account_identity() {
    let fixture = TuiResilienceFixture::new();
    let mut session = fixture.spawn("sparse", BEHAVIOR_RESPONSE_TIMEOUT_MS);

    fixture.wait_for("sparse-reconciled");
    session.wait_until(|| {
        let contents = session.contents();
        contents.contains("Account Identity: developer@example.com")
            && contents.contains("50% used")
    });

    session.quit_and_assert_clean();
}

#[test]
fn a_slow_app_server_that_responds_before_the_deadline_does_not_retry() {
    let fixture = TuiResilienceFixture::new();
    let mut session = fixture.spawn("slow", BEHAVIOR_RESPONSE_TIMEOUT_MS);

    session.wait_until_screen_contains("25% used");
    assert_eq!(fixture.attempt_count(), 1);

    session.quit_and_assert_clean();
    fixture.assert_recorded_children_reaped();
}

struct TuiResilienceFixture {
    root: TempDir,
}

impl TuiResilienceFixture {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("create fixture directory");
        let executable = root.path().join("codex");
        fs::create_dir(root.path().join("state")).expect("create state directory");
        fs::write(
            &executable,
            r#"#!/bin/sh
set -eu

attempt=1
while [ -e "$FAKE_STATE/attempt-$attempt" ]; do
  attempt=$((attempt + 1))
done
touch "$FAKE_STATE/attempt-$attempt"
printf '%s\n' "$$" >> "$FAKE_STATE/pids"

if [ "$FAKE_MODE" = "recover" ] && [ "$attempt" -gt 1 ]; then
  touch "$FAKE_STATE/retry-started"
  while [ ! -e "$FAKE_STATE/allow-recovery" ]; do sleep 0.01; done
fi

read -r message
if [ "$FAKE_MODE" = "hang" ]; then
  sleep 1000 &
  printf '%s\n' "$!" >> "$FAKE_STATE/descendant-pids"
  wait
fi
if [ "$FAKE_MODE" = "slow" ]; then
  sleep 0.25
fi
printf '%s\n' '{"id":0,"result":{"userAgent":"fake","platformFamily":"unix","platformOs":"test"}}'
read -r message
read -r message
printf '%s\n' '{"id":1,"result":{"account":{"type":"chatgpt","email":"developer@example.com","planType":"plus"},"requiresOpenaiAuth":true}}'
read -r message

used=25
if [ "$FAKE_MODE" = "recover" ] && [ "$attempt" -gt 1 ]; then
  used=75
fi
printf '{"id":2,"result":{"rateLimits":{"limitId":"codex","limitName":"Codex","primary":{"usedPercent":%s,"windowDurationMins":60,"resetsAt":4102444800},"secondary":null},"rateLimitsByLimitId":null}}\n' "$used"

case "$FAKE_MODE" in
  recover)
    if [ "$attempt" -eq 1 ]; then
      touch "$FAKE_STATE/first-snapshot"
      while [ ! -e "$FAKE_STATE/allow-disconnect" ]; do sleep 0.01; done
      exit 0
    fi
    ;;
  sparse)
    printf '%s\n' '{"method":"account/rateLimits/updated","params":{"rateLimits":{"primary":{"usedPercent":50}}}}'
    read -r message
    printf '%s\n' '{"id":3,"result":{"rateLimits":{"limitId":"codex","limitName":"Codex","primary":{"usedPercent":50,"windowDurationMins":60,"resetsAt":4102444800},"secondary":null},"rateLimitsByLimitId":null}}'
    touch "$FAKE_STATE/sparse-reconciled"
    ;;
esac

while read -r message; do :; done
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

    fn state(&self) -> std::path::PathBuf {
        self.root.path().join("state")
    }

    fn spawn(&self, mode: &str, response_timeout_ms: u64) -> TuiSession {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 20,
                cols: 90,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("open pseudo-terminal");
        let mut command = CommandBuilder::new(cargo_bin("limitr"));
        let path = std::env::join_paths(std::iter::once(self.root.path().to_path_buf()).chain(
            std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
        ))
        .expect("construct PATH");
        command.env("PATH", path);
        command.env("XDG_CONFIG_HOME", self.root.path().join("config"));
        command.env("CODEX_HOME", self.root.path().join("codex-home"));
        command.env("FAKE_STATE", self.root.path().join("state"));
        command.env("FAKE_MODE", mode);
        command.args([
            "--request-timeout-ms",
            &response_timeout_ms.to_string(),
            "--retry-initial-ms",
            "50",
            "--retry-max-ms",
            "200",
            "--reconcile-interval-ms",
            "5000",
        ]);
        let child = pair
            .slave
            .spawn_command(command)
            .expect("spawn limitr in pseudo-terminal");
        drop(pair.slave);
        let mut reader = pair
            .master
            .try_clone_reader()
            .expect("open terminal output");
        let screen = Arc::new(Mutex::new(vt100::Parser::new(20, 90, 0)));
        let screen_reader = Arc::clone(&screen);
        let drain = thread::spawn(move || {
            let mut buffer = [0_u8; 1024];
            while let Ok(read) = reader.read(&mut buffer) {
                if read == 0 {
                    break;
                }
                screen_reader
                    .lock()
                    .expect("lock terminal parser")
                    .process(&buffer[..read]);
            }
        });
        TuiSession {
            child,
            master: Some(pair.master),
            screen,
            drain: Some(drain),
        }
    }

    fn wait_for(&self, name: &str) {
        let marker = self.state().join(name);
        let deadline = Instant::now() + Duration::from_secs(5);
        while !marker.exists() {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for fixture marker {name}; present markers: {:?}",
                self.state_entries()
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn wait_until(&self, mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !condition() {
            assert!(Instant::now() < deadline, "timed out waiting for condition");
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn attempt_count(&self) -> usize {
        fs::read_dir(self.root.path().join("state"))
            .expect("read state directory")
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().starts_with("attempt-"))
            .count()
    }

    fn state_entries(&self) -> Vec<String> {
        let mut entries: Vec<_> = fs::read_dir(self.state())
            .expect("read state directory")
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        entries.sort();
        entries
    }

    fn assert_recorded_children_reaped(&self) {
        let mut pids = fs::read_to_string(self.root.path().join("state/pids"))
            .expect("read child process ids");
        if let Ok(descendants) = fs::read_to_string(self.root.path().join("state/descendant-pids"))
        {
            pids.push_str(&descendants);
        }
        for pid in pids.lines() {
            assert!(
                !Command::new("kill")
                    .args(["-0", pid])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .expect("check child process")
                    .success(),
                "app-server child {pid} remained alive"
            );
        }
    }
}

struct TuiSession {
    child: Box<dyn portable_pty::Child + Send + Sync>,
    master: Option<Box<dyn portable_pty::MasterPty + Send>>,
    screen: Arc<Mutex<vt100::Parser>>,
    drain: Option<thread::JoinHandle<()>>,
}

impl TuiSession {
    fn contents(&self) -> String {
        self.screen
            .lock()
            .expect("lock terminal parser")
            .screen()
            .contents()
    }

    fn wait_until_screen_contains(&self, expected: &str) {
        self.wait_until(|| self.contents().contains(expected));
    }

    fn wait_until(&self, mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !condition() {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for screen; current contents:\n{}",
                self.contents()
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn quit_and_assert_clean(&mut self) {
        let mut writer = self
            .master
            .as_mut()
            .expect("terminal master")
            .take_writer()
            .expect("open terminal input");
        writer.write_all(b"q\n").expect("send quit key");
        writer.flush().expect("flush quit key");
        let status = self.child.wait().expect("wait for limitr");
        drop(writer);
        drop(self.master.take());
        self.drain
            .take()
            .expect("terminal drain")
            .join()
            .expect("join terminal output reader");
        assert!(status.success(), "limitr exit status: {status:?}");
    }
}
