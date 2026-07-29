use std::collections::BTreeMap;
use std::env;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use chrono::DateTime;
use clap::{Parser, Subcommand};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use thiserror::Error;

#[derive(Debug, Parser)]
#[command(name = "limitr", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Observe the current Limit Snapshot for the default Account Profile.
    Status,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("limitr: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), LimitrError> {
    match Cli::parse().command {
        Commands::Status => print_default_status(),
    }
}

fn print_default_status() -> Result<(), LimitrError> {
    let codex_home = default_codex_home()?;
    let mut app_server = AppServer::start(codex_home)?;

    app_server.initialize()?;
    let identity: AccountReadResult =
        app_server.request(1, "account/read", json!({ "refreshToken": false }))?;
    let rate_limits: RateLimitsReadResult =
        app_server.request(2, "account/rateLimits/read", json!({}))?;

    print!("{}", render_status(identity.account, rate_limits)?);
    Ok(())
}

fn default_codex_home() -> Result<PathBuf, LimitrError> {
    if let Some(codex_home) = env::var_os("CODEX_HOME").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(codex_home));
    }

    home_directory()
        .map(|home| home.join(".codex"))
        .ok_or(LimitrError::CodexHomeUnavailable)
}

#[cfg(unix)]
fn home_directory() -> Option<PathBuf> {
    env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

#[cfg(windows)]
fn home_directory() -> Option<PathBuf> {
    env::var_os("USERPROFILE")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

struct AppServer {
    child: Child,
    input: BufWriter<ChildStdin>,
    output: BufReader<ChildStdout>,
}

impl AppServer {
    fn start(codex_home: PathBuf) -> Result<Self, LimitrError> {
        let mut child = Command::new("codex")
            .args(["app-server", "--stdio"])
            .env("CODEX_HOME", codex_home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(LimitrError::StartAppServer)?;
        let input = child
            .stdin
            .take()
            .ok_or(LimitrError::MissingAppServerPipe)?;
        let output = child
            .stdout
            .take()
            .ok_or(LimitrError::MissingAppServerPipe)?;

        Ok(Self {
            child,
            input: BufWriter::new(input),
            output: BufReader::new(output),
        })
    }

    fn initialize(&mut self) -> Result<(), LimitrError> {
        let _: Value = self.request(
            0,
            "initialize",
            json!({
                "clientInfo": {
                    "name": "limitr",
                    "title": "Limitr",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }),
        )?;
        self.notify("initialized", json!({}))
    }

    fn request<T: DeserializeOwned>(
        &mut self,
        id: u64,
        method: &'static str,
        params: Value,
    ) -> Result<T, LimitrError> {
        self.send(&json!({
            "method": method,
            "id": id,
            "params": params
        }))?;

        loop {
            let response = self.read_message()?;
            if response.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(error) = response.get("error") {
                let message = error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown app-server error");
                return Err(LimitrError::AppServerRequest {
                    method,
                    message: message.into(),
                });
            }
            let result = response
                .get("result")
                .cloned()
                .ok_or(LimitrError::MissingResult(method))?;
            return serde_json::from_value(result)
                .map_err(|source| LimitrError::InvalidAppServerResponse { method, source });
        }
    }

    fn notify(&mut self, method: &'static str, params: Value) -> Result<(), LimitrError> {
        self.send(&json!({
            "method": method,
            "params": params
        }))
    }

    fn send(&mut self, message: &Value) -> Result<(), LimitrError> {
        serde_json::to_writer(&mut self.input, message)?;
        self.input.write_all(b"\n")?;
        self.input.flush()?;
        Ok(())
    }

    fn read_message(&mut self) -> Result<Value, LimitrError> {
        let mut line = String::new();
        if self.output.read_line(&mut line)? == 0 {
            return Err(LimitrError::AppServerClosed);
        }
        serde_json::from_str(&line).map_err(LimitrError::InvalidJson)
    }
}

impl Drop for AppServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AccountReadResult {
    account: Option<AccountIdentity>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AccountIdentity {
    email: Option<String>,
    plan_type: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RateLimitsReadResult {
    rate_limits: Option<LimitBucket>,
    rate_limits_by_limit_id: Option<BTreeMap<String, LimitBucket>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LimitBucket {
    limit_id: String,
    limit_name: Option<String>,
    primary: Option<QuotaWindow>,
    secondary: Option<QuotaWindow>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct QuotaWindow {
    used_percent: f64,
    window_duration_mins: u64,
    resets_at: i64,
}

fn render_status(
    identity: Option<AccountIdentity>,
    rate_limits: RateLimitsReadResult,
) -> Result<String, LimitrError> {
    let mut output = String::from("Profile: default\n");
    if let Some(identity) = identity {
        if let Some(email) = identity.email {
            output.push_str(&format!("Identity: {email}\n"));
        }
        if let Some(plan_type) = identity.plan_type {
            output.push_str(&format!("Plan: {plan_type}\n"));
        }
    }

    let buckets: Vec<LimitBucket> = match rate_limits.rate_limits_by_limit_id {
        Some(buckets) if !buckets.is_empty() => buckets.into_values().collect(),
        Some(_) | None => rate_limits.rate_limits.into_iter().collect(),
    };
    for bucket in buckets {
        output.push('\n');
        match bucket.limit_name.as_deref() {
            Some(name) if name != bucket.limit_id => {
                output.push_str(&format!("Limit Bucket: {name} ({})\n", bucket.limit_id));
            }
            _ => output.push_str(&format!("Limit Bucket: {}\n", bucket.limit_id)),
        }
        if let Some(window) = bucket.primary {
            render_window(&mut output, "Primary", window)?;
        }
        if let Some(window) = bucket.secondary {
            render_window(&mut output, "Secondary", window)?;
        }
    }

    Ok(output)
}

fn render_window(output: &mut String, label: &str, window: QuotaWindow) -> Result<(), LimitrError> {
    let reset = DateTime::from_timestamp(window.resets_at, 0)
        .ok_or(LimitrError::InvalidResetInstant(window.resets_at))?;
    output.push_str(&format!(
        "  {label}: {}% used; {} min window; resets {}\n",
        window.used_percent,
        window.window_duration_mins,
        reset.format("%Y-%m-%dT%H:%M:%SZ")
    ));
    Ok(())
}

#[derive(Debug, Error)]
enum LimitrError {
    #[error("could not determine the normal Codex Home; set CODEX_HOME")]
    CodexHomeUnavailable,
    #[error("could not start `codex app-server --stdio`: {0}")]
    StartAppServer(std::io::Error),
    #[error("the Codex app-server did not provide its standard I/O pipes")]
    MissingAppServerPipe,
    #[error("the Codex app-server closed before returning a Limit Snapshot")]
    AppServerClosed,
    #[error("Codex app-server request `{method}` failed: {message}")]
    AppServerRequest {
        method: &'static str,
        message: String,
    },
    #[error("Codex app-server response to `{0}` did not contain a result")]
    MissingResult(&'static str),
    #[error("Codex app-server returned invalid JSON: {0}")]
    InvalidJson(serde_json::Error),
    #[error("Codex app-server returned an invalid `{method}` result: {source}")]
    InvalidAppServerResponse {
        method: &'static str,
        source: serde_json::Error,
    },
    #[error("Codex app-server returned an invalid Reset Instant: {0}")]
    InvalidResetInstant(i64),
    #[error("could not communicate with the Codex app-server: {0}")]
    Io(#[from] std::io::Error),
    #[error("could not encode a Codex app-server request: {0}")]
    Encode(#[from] serde_json::Error),
}
