use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use thiserror::Error;

pub(crate) struct AppServer {
    child: Child,
    input: BufWriter<ChildStdin>,
    messages: Receiver<Result<Value, AppServerReadError>>,
    reader: Option<JoinHandle<()>>,
    response_timeout: Duration,
}

impl AppServer {
    pub(crate) fn start(
        codex_home: PathBuf,
        response_timeout: Duration,
    ) -> Result<Self, AppServerError> {
        let mut child = Command::new("codex")
            .args(["app-server", "--stdio"])
            .env("CODEX_HOME", codex_home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(AppServerError::Start)?;
        let Some(input) = child.stdin.take() else {
            reap(&mut child);
            return Err(AppServerError::MissingPipe);
        };
        let Some(output) = child.stdout.take() else {
            reap(&mut child);
            return Err(AppServerError::MissingPipe);
        };
        let (message_sender, messages) = mpsc::channel();
        let reader = thread::spawn(move || {
            let mut output = BufReader::new(output);
            loop {
                let mut line = String::new();
                let message = match output.read_line(&mut line) {
                    Ok(0) => Err(AppServerReadError::Closed),
                    Ok(_) => serde_json::from_str(&line).map_err(AppServerReadError::InvalidJson),
                    Err(error) => Err(AppServerReadError::Io(error)),
                };
                let should_stop = message.is_err();
                if message_sender.send(message).is_err() || should_stop {
                    break;
                }
            }
        });

        Ok(Self {
            child,
            input: BufWriter::new(input),
            messages,
            reader: Some(reader),
            response_timeout,
        })
    }

    pub(crate) fn initialize(&mut self) -> Result<(), AppServerError> {
        let _: Value = self.request(0, "initialize", Self::initialize_params())?;
        self.finish_initialize()
    }

    pub(crate) fn initialize_until_stopped(
        &mut self,
        stop: &Receiver<()>,
    ) -> Result<Option<bool>, AppServerError> {
        let Some((_, notification_pending)): Option<(Value, _)> =
            self.request_until_stopped(0, "initialize", Self::initialize_params(), stop)?
        else {
            return Ok(None);
        };
        self.finish_initialize()?;
        Ok(Some(notification_pending))
    }

    pub(crate) fn request<T: DeserializeOwned>(
        &mut self,
        id: u64,
        method: &'static str,
        params: Value,
    ) -> Result<T, AppServerError> {
        self.send(&json!({
            "method": method,
            "id": id,
            "params": params
        }))?;
        let deadline = Instant::now() + self.response_timeout;

        loop {
            let response = self.read_message(deadline, method)?;
            if let Some(value) = Self::decode_response(response, id, method)? {
                return Ok(value);
            }
        }
    }

    pub(crate) fn request_until_stopped<T: DeserializeOwned>(
        &mut self,
        id: u64,
        method: &'static str,
        params: Value,
        stop: &Receiver<()>,
    ) -> Result<Option<(T, bool)>, AppServerError> {
        self.send(&json!({
            "method": method,
            "id": id,
            "params": params
        }))?;
        let deadline = Instant::now() + self.response_timeout;
        let mut notification_pending = false;
        loop {
            if stop.try_recv().is_ok() {
                return Ok(None);
            }
            let remaining = deadline
                .saturating_duration_since(Instant::now())
                .min(Duration::from_millis(100));
            let Some(response) = self.poll_message(remaining)? else {
                if Instant::now() >= deadline {
                    return Err(AppServerError::Timeout(method));
                }
                continue;
            };
            if response.get("id").and_then(Value::as_u64) != Some(id) {
                notification_pending |= response.get("method").and_then(Value::as_str)
                    == Some("account/rateLimits/updated");
                continue;
            }
            let value =
                Self::decode_response(response, id, method)?.expect("response id was checked");
            return Ok(Some((value, notification_pending)));
        }
    }

    pub(crate) fn poll_message(&self, timeout: Duration) -> Result<Option<Value>, AppServerError> {
        self.receive_message(timeout)
    }

    fn receive_message(&self, timeout: Duration) -> Result<Option<Value>, AppServerError> {
        match self.messages.recv_timeout(timeout) {
            Ok(Ok(message)) => Ok(Some(message)),
            Ok(Err(AppServerReadError::Closed)) | Err(RecvTimeoutError::Disconnected) => {
                Err(AppServerError::Closed)
            }
            Ok(Err(AppServerReadError::Io(error))) => Err(AppServerError::Io(error)),
            Ok(Err(AppServerReadError::InvalidJson(error))) => {
                Err(AppServerError::InvalidJson(error))
            }
            Err(RecvTimeoutError::Timeout) => Ok(None),
        }
    }

    fn initialize_params() -> Value {
        json!({
            "clientInfo": {
                "name": "limitr",
                "title": "Limitr",
                "version": env!("CARGO_PKG_VERSION")
            }
        })
    }

    fn finish_initialize(&mut self) -> Result<(), AppServerError> {
        self.notify("initialized", json!({}))
    }

    fn decode_response<T: DeserializeOwned>(
        response: Value,
        id: u64,
        method: &'static str,
    ) -> Result<Option<T>, AppServerError> {
        if response.get("id").and_then(Value::as_u64) != Some(id) {
            return Ok(None);
        }
        if response
            .pointer("/error/code")
            .and_then(Value::as_i64)
            .is_some_and(|code| code == -32601)
        {
            return Err(AppServerError::MissingMethod(method));
        }
        if response.get("error").is_some() {
            return Err(AppServerError::Request(method));
        }
        let result = response
            .get("result")
            .cloned()
            .ok_or(AppServerError::MissingResult(method))?;
        if let Some(field) = missing_required_field(method, &result) {
            return Err(AppServerError::MissingRequiredField { method, field });
        }
        serde_json::from_value(result)
            .map(Some)
            .map_err(|source| AppServerError::InvalidResponse { method, source })
    }

    fn notify(&mut self, method: &'static str, params: Value) -> Result<(), AppServerError> {
        self.send(&json!({
            "method": method,
            "params": params
        }))
    }

    fn send(&mut self, message: &Value) -> Result<(), AppServerError> {
        serde_json::to_writer(&mut self.input, message).map_err(AppServerError::Encode)?;
        self.input.write_all(b"\n")?;
        self.input.flush()?;
        Ok(())
    }

    fn read_message(
        &self,
        deadline: Instant,
        method: &'static str,
    ) -> Result<Value, AppServerError> {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or(AppServerError::Timeout(method))?;
        match self.receive_message(remaining)? {
            Some(message) => Ok(message),
            None => Err(AppServerError::Timeout(method)),
        }
    }
}

impl Drop for AppServer {
    fn drop(&mut self) {
        reap(&mut self.child);
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

fn reap(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn missing_required_field(method: &'static str, result: &Value) -> Option<String> {
    match method {
        "account/read" => {
            if result.get("requiresOpenaiAuth").is_none() {
                return Some("requiresOpenaiAuth".into());
            }
            let Some(account) = result.get("account") else {
                return Some("account".into());
            };
            if account.is_null() {
                return None;
            }
            if account.get("type").is_none() {
                return Some("account.type".into());
            }
            None
        }
        "account/rateLimits/read" => {
            let Some(rate_limits) = result.get("rateLimits") else {
                return Some("rateLimits".into());
            };
            missing_bucket_field("rateLimits", rate_limits).or_else(|| {
                result
                    .get("rateLimitsByLimitId")
                    .and_then(Value::as_object)
                    .and_then(|buckets| {
                        buckets.iter().find_map(|(limit_id, bucket)| {
                            let limit_id = diagnostic_limit_id(limit_id);
                            missing_bucket_field(&format!("rateLimitsByLimitId.{limit_id}"), bucket)
                        })
                    })
            })
        }
        _ => None,
    }
}

fn missing_bucket_field(prefix: &str, bucket: &Value) -> Option<String> {
    ["primary", "secondary"].into_iter().find_map(|window| {
        let value = bucket.get(window)?;
        if value.is_null() {
            return None;
        }
        ["usedPercent", "windowDurationMins", "resetsAt"]
            .into_iter()
            .find(|field| value.get(field).is_none_or(Value::is_null))
            .map(|field| format!("{prefix}.{window}.{field}"))
    })
}

fn diagnostic_limit_id(limit_id: &str) -> String {
    let normalized = limit_id.to_ascii_lowercase();
    if !limit_id.is_empty()
        && limit_id.len() <= 64
        && !["sk-", "token", "secret", "api_key", "apikey"]
            .iter()
            .any(|marker| normalized.contains(marker))
        && limit_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        limit_id.into()
    } else {
        "<redacted>".into()
    }
}

enum AppServerReadError {
    Closed,
    Io(std::io::Error),
    InvalidJson(serde_json::Error),
}

#[derive(Debug, Error)]
pub(crate) enum AppServerError {
    #[error("could not start `codex app-server --stdio`: {0}")]
    Start(std::io::Error),
    #[error("the Codex app-server did not provide its standard I/O pipes")]
    MissingPipe,
    #[error("the Codex app-server closed before returning a Limit Snapshot")]
    Closed,
    #[error("Codex app-server request `{0}` timed out")]
    Timeout(&'static str),
    #[error(
        "Codex app-server request `{0}` failed; check the Account Profile authentication and Codex compatibility"
    )]
    Request(&'static str),
    #[error("incompatible Codex app-server: required method `{0}` is unavailable; update Codex")]
    MissingMethod(&'static str),
    #[error(
        "incompatible Codex app-server response to `{method}`: missing required field `{field}`; update Codex"
    )]
    MissingRequiredField { method: &'static str, field: String },
    #[error(
        "incompatible Codex app-server response to `{0}`: missing required `result`; update Codex"
    )]
    MissingResult(&'static str),
    #[error("Codex app-server returned invalid JSON")]
    InvalidJson(#[source] serde_json::Error),
    #[error(
        "incompatible Codex app-server response to `{method}`; required fields have changed; update Codex"
    )]
    InvalidResponse {
        method: &'static str,
        #[source]
        source: serde_json::Error,
    },
    #[error("could not communicate with the Codex app-server: {0}")]
    Io(#[from] std::io::Error),
    #[error("could not encode a Codex app-server request")]
    Encode(#[source] serde_json::Error),
}

impl AppServerError {
    pub(crate) fn is_retryable(&self) -> bool {
        !matches!(
            self,
            Self::MissingMethod(_)
                | Self::MissingRequiredField { .. }
                | Self::MissingResult(_)
                | Self::InvalidResponse { .. }
        )
    }
}
