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

#[test]
fn default_command_quits_with_q_and_stops_its_app_server_child() {
    let fixture = InteractiveFixture::new();
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("open pseudo-terminal");
    let mut command = CommandBuilder::new(cargo_bin("limitr"));
    fixture.configure(&mut command);
    let mut child = pair
        .slave
        .spawn_command(command)
        .expect("spawn limitr in pseudo-terminal");
    drop(pair.slave);

    fixture.wait_for(&fixture.pid_file);
    let pid = fs::read_to_string(&fixture.pid_file).expect("read fake app-server pid");
    let mut reader = pair
        .master
        .try_clone_reader()
        .expect("open terminal output");
    let mut started = [0_u8; 1];
    reader
        .read_exact(&mut started)
        .expect("wait for TUI output");
    let drain = thread::spawn(move || {
        let _ = std::io::copy(&mut reader, &mut std::io::sink());
    });
    let mut writer = pair.master.take_writer().expect("open terminal input");
    writer.write_all(b"q\n").expect("send quit key");
    writer.flush().expect("flush quit key");

    let status = child.wait().expect("wait for limitr");
    drop(writer);
    drop(pair.master);
    drain.join().expect("join terminal output reader");
    assert!(status.success(), "limitr exit status: {status:?}");
    assert!(
        !Command::new("kill")
            .args(["-0", pid.trim()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("check fake app-server process")
            .success(),
        "app-server child {pid} remained alive"
    );
}

#[test]
fn rate_limit_notifications_trigger_prompt_reconciliation() {
    let fixture = InteractiveFixture::new();
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("open pseudo-terminal");
    let mut command = CommandBuilder::new(cargo_bin("limitr"));
    fixture.configure(&mut command);
    command.env("FAKE_SEND_SECOND_NOTIFICATION", "1");
    let mut child = pair
        .slave
        .spawn_command(command)
        .expect("spawn limitr in pseudo-terminal");
    drop(pair.slave);
    let mut reader = pair
        .master
        .try_clone_reader()
        .expect("open terminal output");
    let screen = Arc::new(Mutex::new(vt100::Parser::new(24, 80, 0)));
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

    fixture.wait_for(&fixture.reconciled_file);
    fixture.wait_for(&fixture.second_reconciled_file);
    fixture.wait_until(|| {
        screen
            .lock()
            .expect("lock terminal parser")
            .screen()
            .contents()
            .contains("75% used")
    });
    let mut writer = pair.master.take_writer().expect("open terminal input");
    writer.write_all(b"q\n").expect("send quit key");
    writer.flush().expect("flush quit key");

    let status = child.wait().expect("wait for limitr");
    drop(writer);
    drop(pair.master);
    drain.join().expect("join terminal output reader");
    assert!(status.success(), "limitr exit status: {status:?}");
}

#[test]
fn periodic_reads_reconcile_when_no_notification_arrives() {
    let fixture = InteractiveFixture::new();
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 12,
            cols: 48,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("open pseudo-terminal");
    let mut command = CommandBuilder::new(cargo_bin("limitr"));
    fixture.configure(&mut command);
    command.env("FAKE_SEND_NOTIFICATION", "0");
    command.args(["--reconcile-interval-ms", "100"]);
    let mut child = pair
        .slave
        .spawn_command(command)
        .expect("spawn limitr in pseudo-terminal");
    drop(pair.slave);
    let mut reader = pair
        .master
        .try_clone_reader()
        .expect("open terminal output");
    let drain = thread::spawn(move || {
        let _ = std::io::copy(&mut reader, &mut std::io::sink());
    });

    fixture.wait_for(&fixture.reconciled_file);
    let mut writer = pair.master.take_writer().expect("open terminal input");
    writer.write_all(b"\x1b").expect("send escape key");
    writer.flush().expect("flush escape key");

    let status = child.wait().expect("wait for limitr");
    drop(writer);
    drop(pair.master);
    drain.join().expect("join terminal output reader");
    assert!(status.success(), "limitr exit status: {status:?}");
}

#[test]
fn quit_interrupts_an_unresponsive_startup_request_and_cleans_up_the_child() {
    let fixture = InteractiveFixture::new();
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 12,
            cols: 48,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("open pseudo-terminal");
    let mut command = CommandBuilder::new(cargo_bin("limitr"));
    fixture.configure(&mut command);
    command.env("FAKE_STALL_INITIALIZE", "1");
    let mut child = pair
        .slave
        .spawn_command(command)
        .expect("spawn limitr in pseudo-terminal");
    drop(pair.slave);
    fixture.wait_for(&fixture.pid_file);
    let pid = fs::read_to_string(&fixture.pid_file).expect("read fake app-server pid");
    let mut reader = pair
        .master
        .try_clone_reader()
        .expect("open terminal output");
    let mut started = [0_u8; 1];
    reader
        .read_exact(&mut started)
        .expect("wait for TUI output");
    let drain = thread::spawn(move || {
        let _ = std::io::copy(&mut reader, &mut std::io::sink());
    });
    let mut writer = pair.master.take_writer().expect("open terminal input");
    let quit_at = Instant::now();
    writer.write_all(b"\x03").expect("send Ctrl-C");
    writer.flush().expect("flush Ctrl-C");

    let status = child.wait().expect("wait for limitr");
    assert!(quit_at.elapsed() < Duration::from_secs(2));
    drop(writer);
    drop(pair.master);
    drain.join().expect("join terminal output reader");
    assert!(status.success(), "limitr exit status: {status:?}");
    assert!(
        !Command::new("kill")
            .args(["-0", pid.trim()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("check fake app-server process")
            .success(),
        "app-server child {pid} remained alive"
    );
}

#[test]
fn malformed_reset_instants_surface_a_profile_error_instead_of_hiding_the_window() {
    let fixture = InteractiveFixture::new();
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 16,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("open pseudo-terminal");
    let mut command = CommandBuilder::new(cargo_bin("limitr"));
    fixture.configure(&mut command);
    command.env("FAKE_INVALID_RESET", "1");
    command.env("FAKE_SEND_NOTIFICATION", "0");
    let mut child = pair
        .slave
        .spawn_command(command)
        .expect("spawn limitr in pseudo-terminal");
    drop(pair.slave);
    let mut reader = pair
        .master
        .try_clone_reader()
        .expect("open terminal output");
    let screen = Arc::new(Mutex::new(vt100::Parser::new(16, 80, 0)));
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

    fixture.wait_until(|| {
        screen
            .lock()
            .expect("lock terminal parser")
            .screen()
            .contents()
            .contains("invalid Reset Instant")
    });
    let mut writer = pair.master.take_writer().expect("open terminal input");
    writer.write_all(b"q\n").expect("send quit key");
    writer.flush().expect("flush quit key");

    let status = child.wait().expect("wait for limitr");
    drop(writer);
    drop(pair.master);
    drain.join().expect("join terminal output reader");
    assert!(status.success(), "limitr exit status: {status:?}");
}

struct InteractiveFixture {
    root: TempDir,
    pid_file: std::path::PathBuf,
    reconciled_file: std::path::PathBuf,
    second_reconciled_file: std::path::PathBuf,
}

impl InteractiveFixture {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("create fixture");
        let executable = root.path().join("codex");
        fs::write(
            &executable,
            r#"#!/bin/sh
set -eu
echo "$$" > "$FAKE_PID_FILE"

read -r message
if [ "${FAKE_STALL_INITIALIZE:-0}" = "1" ]; then
  read -r never
fi
printf '%s\n' '{"id":0,"result":{"userAgent":"fake","platformFamily":"unix","platformOs":"test"}}'
read -r message
read -r message
printf '%s\n' '{"id":1,"result":{"account":{"type":"chatgpt","email":"developer@example.com","planType":"plus"}}}'
read -r message
reset=4102444800
if [ "${FAKE_INVALID_RESET:-0}" = "1" ]; then
  reset=9223372036854775807
fi
printf '{"id":2,"result":{"rateLimits":{"limitId":"codex","limitName":"Codex","primary":{"usedPercent":25,"windowDurationMins":60,"resetsAt":%s},"secondary":null},"rateLimitsByLimitId":null}}\n' "$reset"
if [ "${FAKE_SEND_NOTIFICATION:-1}" = "1" ]; then
  printf '%s\n' '{"method":"account/rateLimits/updated","params":{"rateLimits":{"primary":{"usedPercent":50}}}}'
fi
read -r message
touch "$FAKE_RECONCILED_FILE"
if [ "${FAKE_SEND_SECOND_NOTIFICATION:-0}" = "1" ]; then
  printf '%s\n' '{"method":"account/rateLimits/updated","params":{"rateLimits":{"primary":{"usedPercent":75}}}}'
fi
printf '%s\n' '{"id":3,"result":{"rateLimits":{"limitId":"codex","limitName":"Codex","primary":{"usedPercent":50,"windowDurationMins":60,"resetsAt":4102444800},"secondary":null},"rateLimitsByLimitId":null}}'
if [ "${FAKE_SEND_SECOND_NOTIFICATION:-0}" = "1" ]; then
  read -r message
  touch "$FAKE_SECOND_RECONCILED_FILE"
  printf '%s\n' '{"id":4,"result":{"rateLimits":{"limitId":"codex","limitName":"Codex","primary":{"usedPercent":75,"windowDurationMins":60,"resetsAt":4102444800},"secondary":null},"rateLimitsByLimitId":null}}'
fi
while read -r message; do :; done
"#,
        )
        .expect("write fake codex");
        let mut permissions = fs::metadata(&executable)
            .expect("read fake codex metadata")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&executable, permissions).expect("make fake codex executable");
        let pid_file = root.path().join("app-server.pid");
        let reconciled_file = root.path().join("reconciled");
        let second_reconciled_file = root.path().join("second-reconciled");
        Self {
            root,
            pid_file,
            reconciled_file,
            second_reconciled_file,
        }
    }

    fn configure(&self, command: &mut CommandBuilder) {
        let path = std::env::join_paths(std::iter::once(self.root.path().to_path_buf()).chain(
            std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
        ))
        .expect("construct PATH");
        command.env("PATH", path);
        command.env("XDG_CONFIG_HOME", self.root.path().join("config"));
        command.env("CODEX_HOME", self.root.path().join("codex-home"));
        command.env("FAKE_PID_FILE", &self.pid_file);
        command.env("FAKE_RECONCILED_FILE", &self.reconciled_file);
        command.env("FAKE_SECOND_RECONCILED_FILE", &self.second_reconciled_file);
    }

    fn wait_for(&self, path: &std::path::Path) {
        self.wait_until(|| path.exists());
    }

    fn wait_until(&self, mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !condition() {
            assert!(Instant::now() < deadline, "timed out waiting for condition");
            thread::sleep(Duration::from_millis(10));
        }
    }
}
