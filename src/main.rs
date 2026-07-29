mod config;

use std::collections::BTreeMap;
use std::env;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use chrono::DateTime;
use clap::{Parser, Subcommand};
use config::{AccountProfile, Config, config_file};
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
    /// Observe the current Limit Snapshot for every Account Profile.
    Status,
    /// Manage configured Account Profiles.
    Profile {
        #[command(subcommand)]
        command: ProfileCommand,
    },
}

#[derive(Debug, Subcommand)]
enum ProfileCommand {
    /// Add an Account Profile.
    Add {
        /// Unique local label for the Account Profile.
        label: String,
        /// Absolute path to the profile's Codex Home.
        codex_home: PathBuf,
    },
    /// List configured Account Profiles.
    List,
    /// Remove an Account Profile without modifying its Codex Home.
    Remove {
        /// Label of the Account Profile to remove.
        label: String,
    },
}

fn main() {
    match run() {
        Ok(0) => {}
        Ok(exit_status) => std::process::exit(exit_status),
        Err(error) => {
            eprintln!("limitr: {error}");
            std::process::exit(1);
        }
    }
}

fn run() -> Result<i32, LimitrError> {
    match Cli::parse().command {
        Commands::Status => print_status(),
        Commands::Profile { command } => {
            manage_profiles(command)?;
            Ok(0)
        }
    }
}

fn manage_profiles(command: ProfileCommand) -> Result<(), LimitrError> {
    let path = config_file()?;
    let mut config = Config::load(&path)?.unwrap_or_default();
    match command {
        ProfileCommand::Add { label, codex_home } => {
            config.add(label, codex_home)?;
            config.save(&path)?;
            Ok(())
        }
        ProfileCommand::List => {
            for profile in config.profiles() {
                println!("{}\t{}", profile.label, profile.codex_home.display());
            }
            Ok(())
        }
        ProfileCommand::Remove { label } => {
            config.remove(&label)?;
            config.save(&path)?;
            Ok(())
        }
    }
}

fn print_status() -> Result<i32, LimitrError> {
    let (profiles, synthesized_default) = match Config::load(&config_file()?)? {
        Some(config) => (config.into_profiles(), false),
        None => (
            vec![AccountProfile {
                label: "default".into(),
                codex_home: default_codex_home()?,
            }],
            true,
        ),
    };

    let observations: Vec<_> = profiles
        .into_iter()
        .map(|profile| {
            thread::spawn(move || {
                let result = observe_profile(&profile.codex_home);
                (profile.label, result)
            })
        })
        .collect();
    let mut observations = observations
        .into_iter()
        .map(|observation| {
            let (label, result) = observation
                .join()
                .map_err(|_| LimitrError::ObservationWorkerPanicked)?;
            Ok((label, result))
        })
        .collect::<Result<Vec<_>, LimitrError>>()?;
    if synthesized_default {
        let (label, observation) = observations.pop().expect("one observation");
        match observation {
            Ok(snapshot) => observations.push((label, Ok(snapshot))),
            Err(error) => return Err(error),
        }
    }
    let mut identity_profiles: BTreeMap<AccountIdentityKey, Vec<String>> = BTreeMap::new();
    for (label, observation) in &observations {
        if let Ok(snapshot) = observation {
            if let Some(identity_key) = snapshot
                .identity
                .as_ref()
                .and_then(AccountIdentity::comparison_key)
            {
                identity_profiles
                    .entry(identity_key)
                    .or_default()
                    .push(label.clone());
            }
        }
    }
    let failure_count = observations
        .iter()
        .filter(|(_, observation)| observation.is_err())
        .count();
    let rendered = observations
        .into_iter()
        .map(|(label, observation)| match observation {
            Ok(snapshot) => {
                let duplicates = snapshot
                    .identity
                    .as_ref()
                    .and_then(AccountIdentity::comparison_key)
                    .and_then(|identity_key| identity_profiles.get(&identity_key))
                    .filter(|profiles| profiles.len() > 1)
                    .map(Vec::as_slice);
                render_status(&label, snapshot.identity, snapshot.rate_limits, duplicates)
            }
            Err(error) => Ok(format!("Account Profile: {label}\nError: {error}\n")),
        })
        .collect::<Result<Vec<_>, _>>()?;
    print!("{}", rendered.join("\n"));
    Ok(if failure_count == 0 { 0 } else { 2 })
}

fn observe_profile(codex_home: &Path) -> Result<LimitSnapshot, LimitrError> {
    let mut app_server = AppServer::start(codex_home.to_path_buf())?;
    app_server.initialize()?;
    let identity: AccountReadResult =
        app_server.request(1, "account/read", json!({ "refreshToken": false }))?;
    let rate_limits: RateLimitsReadResult =
        app_server.request(2, "account/rateLimits/read", json!({}))?;

    Ok(LimitSnapshot {
        identity: identity.account,
        rate_limits,
    })
}

struct LimitSnapshot {
    identity: Option<AccountIdentity>,
    rate_limits: RateLimitsReadResult,
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
    messages: Receiver<Result<Value, AppServerReadError>>,
    reader: Option<JoinHandle<()>>,
}

impl AppServer {
    const RESPONSE_TIMEOUT: Duration = Duration::from_secs(10);

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
        let deadline = Instant::now() + Self::RESPONSE_TIMEOUT;

        loop {
            let response = self.read_message(deadline, method)?;
            if response.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if response.get("error").is_some() {
                return Err(LimitrError::AppServerRequest(method));
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

    fn read_message(&self, deadline: Instant, method: &'static str) -> Result<Value, LimitrError> {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or(LimitrError::AppServerTimeout(method))?;
        match self.messages.recv_timeout(remaining) {
            Ok(Ok(message)) => Ok(message),
            Ok(Err(AppServerReadError::Closed)) | Err(RecvTimeoutError::Disconnected) => {
                Err(LimitrError::AppServerClosed)
            }
            Ok(Err(AppServerReadError::Io(error))) => Err(LimitrError::Io(error)),
            Ok(Err(AppServerReadError::InvalidJson(error))) => Err(LimitrError::InvalidJson(error)),
            Err(RecvTimeoutError::Timeout) => Err(LimitrError::AppServerTimeout(method)),
        }
    }
}

impl Drop for AppServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

enum AppServerReadError {
    Closed,
    Io(std::io::Error),
    InvalidJson(serde_json::Error),
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

impl AccountIdentity {
    fn comparison_key(&self) -> Option<AccountIdentityKey> {
        self.email.clone().map(AccountIdentityKey)
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct AccountIdentityKey(String);

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RateLimitsReadResult {
    rate_limits: LimitBucket,
    rate_limits_by_limit_id: Option<BTreeMap<String, LimitBucket>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LimitBucket {
    limit_id: Option<String>,
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
    label: &str,
    identity: Option<AccountIdentity>,
    rate_limits: RateLimitsReadResult,
    duplicate_profiles: Option<&[String]>,
) -> Result<String, LimitrError> {
    let mut output = format!("Account Profile: {label}\n");
    if let Some(identity) = identity {
        if let Some(email) = identity.email {
            output.push_str(&format!("Account Identity: {email}\n"));
        }
        if let Some(plan_type) = identity.plan_type {
            output.push_str(&format!("Plan: {plan_type}\n"));
        }
    }
    if let Some(profiles) = duplicate_profiles {
        output.push_str(&format!(
            "Duplicate Account Identity: Account Profiles {}\n",
            profiles.join(", ")
        ));
    }

    let buckets: Vec<(String, LimitBucket)> = match rate_limits.rate_limits_by_limit_id {
        Some(buckets) if !buckets.is_empty() => buckets.into_iter().collect(),
        Some(_) | None => {
            let bucket = rate_limits.rate_limits;
            let limit_id = bucket.limit_id.clone().unwrap_or_else(|| "codex".into());
            vec![(limit_id, bucket)]
        }
    };
    for (limit_id, bucket) in buckets {
        output.push('\n');
        match bucket.limit_name.as_deref() {
            Some(name) if name != limit_id => {
                output.push_str(&format!("Limit Bucket: {name} ({limit_id})\n"));
            }
            _ => output.push_str(&format!("Limit Bucket: {limit_id}\n")),
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
    #[error(transparent)]
    Config(#[from] config::ConfigError),
    #[error("an Account Profile observation worker stopped unexpectedly")]
    ObservationWorkerPanicked,
    #[error("could not determine the normal Codex Home; set CODEX_HOME")]
    CodexHomeUnavailable,
    #[error("could not start `codex app-server --stdio`: {0}")]
    StartAppServer(std::io::Error),
    #[error("the Codex app-server did not provide its standard I/O pipes")]
    MissingAppServerPipe,
    #[error("the Codex app-server closed before returning a Limit Snapshot")]
    AppServerClosed,
    #[error("Codex app-server request `{0}` timed out")]
    AppServerTimeout(&'static str),
    #[error(
        "Codex app-server request `{0}` failed; check the Account Profile authentication and Codex compatibility"
    )]
    AppServerRequest(&'static str),
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
