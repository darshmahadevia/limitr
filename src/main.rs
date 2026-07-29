mod config;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::env;
use std::io::{BufRead, BufReader, BufWriter, Stdout, Write, stdout};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use chrono::{DateTime, Local};
use clap::{Parser, Subcommand};
use config::{AccountProfile, Config, config_file};
use crossterm::cursor::{Hide, Show};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use limitr::tui::{LimitBucketView, LiveTrace, ProfileView, QuotaWindowView, render_view_state};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use thiserror::Error;

#[derive(Debug, Parser)]
#[command(name = "limitr", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
    /// Milliseconds between fallback rate-limit reconciliation reads.
    #[arg(long, default_value_t = 30_000, hide = true)]
    reconcile_interval_ms: u64,
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
    let cli = Cli::parse();
    match cli.command {
        Some(Commands::Status) => print_status(),
        Some(Commands::Profile { command }) => {
            manage_profiles(command)?;
            Ok(0)
        }
        None => {
            run_interactive(Duration::from_millis(cli.reconcile_interval_ms.max(1)))?;
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

fn run_interactive(reconcile_interval: Duration) -> Result<(), LimitrError> {
    let profiles = load_account_profiles()?.profiles;
    let mut monitored: Vec<_> = profiles
        .into_iter()
        .map(|profile| MonitoredProfile::start(profile, reconcile_interval))
        .collect();
    let mut terminal = TerminalSession::start()?;
    let ascii =
        env::var_os("NO_COLOR").is_some() || env::var("TERM").is_ok_and(|term| term == "dumb");
    let mut scroll = 0_u16;

    loop {
        for profile in &mut monitored {
            profile.receive_events();
        }
        let views: Vec<_> = monitored
            .iter()
            .map(|profile| profile.view.clone())
            .collect();
        scroll = terminal.draw(&views, Local::now().fixed_offset(), ascii, scroll)?;

        if !event::poll(Duration::from_millis(100))? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => break,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => break,
            KeyCode::Up | KeyCode::Char('k') => scroll = scroll.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => scroll = scroll.saturating_add(1),
            KeyCode::PageUp => scroll = scroll.saturating_sub(5),
            KeyCode::PageDown => scroll = scroll.saturating_add(5),
            _ => {}
        }
    }
    Ok(())
}

struct LoadedProfiles {
    profiles: Vec<AccountProfile>,
    synthesized_default: bool,
}

fn load_account_profiles() -> Result<LoadedProfiles, LimitrError> {
    Ok(match Config::load(&config_file()?)? {
        Some(config) => LoadedProfiles {
            profiles: config.into_profiles(),
            synthesized_default: false,
        },
        None => LoadedProfiles {
            profiles: vec![AccountProfile {
                label: "default".into(),
                codex_home: default_codex_home()?,
            }],
            synthesized_default: true,
        },
    })
}

struct TerminalSession {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl TerminalSession {
    fn start() -> Result<Self, LimitrError> {
        enable_raw_mode()?;
        let mut output = stdout();
        if let Err(error) = execute!(output, EnterAlternateScreen, Hide) {
            let _ = disable_raw_mode();
            return Err(error.into());
        }
        let terminal = match Terminal::new(CrosstermBackend::new(output)) {
            Ok(terminal) => terminal,
            Err(error) => {
                let mut output = stdout();
                let _ = execute!(output, Show, LeaveAlternateScreen);
                let _ = disable_raw_mode();
                return Err(error.into());
            }
        };
        Ok(Self { terminal })
    }

    fn draw(
        &mut self,
        profiles: &[ProfileView],
        now: DateTime<chrono::FixedOffset>,
        ascii: bool,
        scroll: u16,
    ) -> Result<u16, LimitrError> {
        let area = self.terminal.size()?;
        let width = area.width.saturating_sub(2);
        let height = area.height.saturating_sub(2);
        let rendered = render_view_state(profiles, now, width, height, ascii, scroll);
        let normalized_scroll = rendered.scroll;
        self.terminal.draw(|frame| {
            let paragraph = Paragraph::new(rendered.text.as_str())
                .block(Block::default().borders(Borders::ALL).title(" Limitr "))
                .wrap(Wrap { trim: false });
            frame.render_widget(paragraph, frame.area());
        })?;
        Ok(normalized_scroll)
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(self.terminal.backend_mut(), Show, LeaveAlternateScreen);
        let _ = self.terminal.show_cursor();
    }
}

struct MonitoredProfile {
    view: ProfileView,
    traces: HashMap<TraceKey, LiveTrace>,
    events: Receiver<MonitorEvent>,
    stop: Sender<()>,
    worker: Option<JoinHandle<()>>,
}

impl MonitoredProfile {
    fn start(profile: AccountProfile, reconcile_interval: Duration) -> Self {
        let (event_sender, events) = mpsc::channel();
        let (stop, stop_receiver) = mpsc::channel();
        let label = profile.label.clone();
        let worker = thread::spawn(move || {
            monitor_profile(
                profile.codex_home,
                reconcile_interval,
                event_sender,
                stop_receiver,
            );
        });
        Self {
            view: ProfileView {
                label,
                identity: None,
                plan: None,
                buckets: Vec::new(),
                error: None,
            },
            traces: HashMap::new(),
            events,
            stop,
            worker: Some(worker),
        }
    }

    fn receive_events(&mut self) {
        loop {
            match self.events.try_recv() {
                Ok(MonitorEvent::Snapshot(snapshot)) => self.apply_snapshot(snapshot),
                Ok(MonitorEvent::Error(error)) => {
                    self.view.buckets.clear();
                    self.view.error = Some(error);
                }
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
    }

    fn apply_snapshot(&mut self, snapshot: LimitSnapshot) {
        let identity = self.view.identity.take();
        let plan = self.view.plan.take();
        match profile_view(self.view.label.clone(), snapshot, &mut self.traces) {
            Ok(mut view) => {
                if view.identity.is_none() {
                    view.identity = identity;
                    view.plan = plan;
                }
                self.view = view;
            }
            Err(error) => {
                self.view.identity = identity;
                self.view.plan = plan;
                self.view.buckets.clear();
                self.view.error = Some(error.to_string());
            }
        }
    }
}

impl Drop for MonitoredProfile {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

enum MonitorEvent {
    Snapshot(LimitSnapshot),
    Error(String),
}

fn monitor_profile(
    codex_home: PathBuf,
    reconcile_interval: Duration,
    events: Sender<MonitorEvent>,
    stop: Receiver<()>,
) {
    if let Err(error) =
        monitor_profile_until_stopped(codex_home, reconcile_interval, &events, &stop)
    {
        let _ = events.send(MonitorEvent::Error(error.to_string()));
    }
}

fn monitor_profile_until_stopped(
    codex_home: PathBuf,
    reconcile_interval: Duration,
    events: &Sender<MonitorEvent>,
    stop: &Receiver<()>,
) -> Result<(), LimitrError> {
    let mut app_server = AppServer::start(codex_home)?;
    let Some(mut notification_pending) = app_server.initialize_until_stopped(stop)? else {
        return Ok(());
    };
    let Some((identity, notification_during_identity)): Option<(AccountReadResult, _)> = app_server
        .request_until_stopped(1, "account/read", json!({ "refreshToken": false }), stop)?
    else {
        return Ok(());
    };
    notification_pending |= notification_during_identity;
    let Some((rate_limits, notification_during_limits)): Option<(RateLimitsReadResult, _)> =
        app_server.request_until_stopped(2, "account/rateLimits/read", json!({}), stop)?
    else {
        return Ok(());
    };
    notification_pending |= notification_during_limits;
    events
        .send(MonitorEvent::Snapshot(LimitSnapshot {
            identity: identity.account,
            rate_limits,
        }))
        .map_err(|_| LimitrError::MonitorClosed)?;

    let mut request_id = 3;
    let mut next_reconciliation = if notification_pending {
        Instant::now()
    } else {
        Instant::now() + reconcile_interval
    };
    loop {
        if stop.try_recv().is_ok() {
            return Ok(());
        }
        let now = Instant::now();
        let notification = app_server.poll_message(
            next_reconciliation
                .saturating_duration_since(now)
                .min(Duration::from_millis(100)),
        )?;
        let should_reconcile = notification.as_ref().is_some_and(|message| {
            message.get("method").and_then(Value::as_str) == Some("account/rateLimits/updated")
        }) || Instant::now() >= next_reconciliation;
        if !should_reconcile {
            continue;
        }

        let Some((rate_limits, notification_pending)) = app_server.request_until_stopped(
            request_id,
            "account/rateLimits/read",
            json!({}),
            stop,
        )?
        else {
            return Ok(());
        };
        request_id += 1;
        next_reconciliation = if notification_pending {
            Instant::now()
        } else {
            Instant::now() + reconcile_interval
        };
        events
            .send(MonitorEvent::Snapshot(LimitSnapshot {
                identity: None,
                rate_limits,
            }))
            .map_err(|_| LimitrError::MonitorClosed)?;
    }
}

fn profile_view(
    label: String,
    snapshot: LimitSnapshot,
    traces: &mut HashMap<TraceKey, LiveTrace>,
) -> Result<ProfileView, LimitrError> {
    let (identity, plan) = snapshot.identity.map_or((None, None), |identity| {
        (identity.email, identity.plan_type)
    });
    let buckets = limit_bucket_views(snapshot.rate_limits, traces)?;
    Ok(ProfileView {
        label,
        identity,
        plan,
        buckets,
        error: None,
    })
}

fn limit_bucket_views(
    rate_limits: RateLimitsReadResult,
    traces: &mut HashMap<TraceKey, LiveTrace>,
) -> Result<Vec<LimitBucketView>, LimitrError> {
    let buckets: Vec<(String, LimitBucket)> = match rate_limits.rate_limits_by_limit_id {
        Some(buckets) if !buckets.is_empty() => buckets.into_iter().collect(),
        Some(_) | None => {
            let bucket = rate_limits.rate_limits;
            let limit_id = bucket.limit_id.clone().unwrap_or_else(|| "codex".into());
            vec![(limit_id, bucket)]
        }
    };
    let mut bucket_views = Vec::new();
    let mut active_traces = HashSet::new();
    for (limit_id, bucket) in buckets {
        let bucket_name = match bucket.limit_name {
            Some(name) if name != limit_id => format!("{name} ({limit_id})"),
            _ => limit_id.clone(),
        };
        let mut windows = Vec::new();
        for (window_kind, window) in [
            (QuotaWindowKind::Primary, bucket.primary),
            (QuotaWindowKind::Secondary, bucket.secondary),
        ] {
            let Some(window) = window else {
                continue;
            };
            let reset = DateTime::from_timestamp(window.resets_at, 0)
                .ok_or(LimitrError::InvalidResetInstant(window.resets_at))?;
            let key = TraceKey {
                limit_id: limit_id.clone(),
                window_kind,
            };
            active_traces.insert(key.clone());
            let trace = traces.entry(key).or_default();
            trace.push(window.used_percent);
            windows.push(QuotaWindowView {
                window: window_kind.label().into(),
                used_percent: window.used_percent,
                window_duration_mins: window.window_duration_mins,
                resets_at: reset.with_timezone(&Local).fixed_offset(),
                trace: trace.clone(),
            });
        }
        bucket_views.push(LimitBucketView {
            label: bucket_name,
            windows,
        });
    }
    traces.retain(|key, _| active_traces.contains(key));
    Ok(bucket_views)
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum QuotaWindowKind {
    Primary,
    Secondary,
}

impl QuotaWindowKind {
    fn label(self) -> &'static str {
        match self {
            Self::Primary => "Primary",
            Self::Secondary => "Secondary",
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct TraceKey {
    limit_id: String,
    window_kind: QuotaWindowKind,
}

fn print_status() -> Result<i32, LimitrError> {
    let LoadedProfiles {
        profiles,
        synthesized_default,
    } = load_account_profiles()?;

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
                let mut traces = HashMap::new();
                let view = profile_view(label, snapshot, &mut traces)?;
                Ok::<_, LimitrError>(render_status(&view, duplicates))
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
        let _: Value = self.request(0, "initialize", Self::initialize_params())?;
        self.finish_initialize()
    }

    fn initialize_until_stopped(
        &mut self,
        stop: &Receiver<()>,
    ) -> Result<Option<bool>, LimitrError> {
        let Some((_, notification_pending)): Option<(Value, _)> =
            self.request_until_stopped(0, "initialize", Self::initialize_params(), stop)?
        else {
            return Ok(None);
        };
        self.finish_initialize()?;
        Ok(Some(notification_pending))
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

    fn finish_initialize(&mut self) -> Result<(), LimitrError> {
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
            if let Some(value) = Self::decode_response(response, id, method)? {
                return Ok(value);
            }
        }
    }

    fn request_until_stopped<T: DeserializeOwned>(
        &mut self,
        id: u64,
        method: &'static str,
        params: Value,
        stop: &Receiver<()>,
    ) -> Result<Option<(T, bool)>, LimitrError> {
        self.send(&json!({
            "method": method,
            "id": id,
            "params": params
        }))?;
        let deadline = Instant::now() + Self::RESPONSE_TIMEOUT;
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
                    return Err(LimitrError::AppServerTimeout(method));
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

    fn decode_response<T: DeserializeOwned>(
        response: Value,
        id: u64,
        method: &'static str,
    ) -> Result<Option<T>, LimitrError> {
        if response.get("id").and_then(Value::as_u64) != Some(id) {
            return Ok(None);
        }
        if response.get("error").is_some() {
            return Err(LimitrError::AppServerRequest(method));
        }
        let result = response
            .get("result")
            .cloned()
            .ok_or(LimitrError::MissingResult(method))?;
        serde_json::from_value(result)
            .map(Some)
            .map_err(|source| LimitrError::InvalidAppServerResponse { method, source })
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

    fn poll_message(&self, timeout: Duration) -> Result<Option<Value>, LimitrError> {
        match self.messages.recv_timeout(timeout) {
            Ok(Ok(message)) => Ok(Some(message)),
            Ok(Err(AppServerReadError::Closed)) | Err(RecvTimeoutError::Disconnected) => {
                Err(LimitrError::AppServerClosed)
            }
            Ok(Err(AppServerReadError::Io(error))) => Err(LimitrError::Io(error)),
            Ok(Err(AppServerReadError::InvalidJson(error))) => Err(LimitrError::InvalidJson(error)),
            Err(RecvTimeoutError::Timeout) => Ok(None),
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

fn render_status(view: &ProfileView, duplicate_profiles: Option<&[String]>) -> String {
    let mut output = format!("Account Profile: {}\n", view.label);
    if let Some(identity) = &view.identity {
        output.push_str(&format!("Account Identity: {identity}\n"));
    }
    if let Some(plan) = &view.plan {
        output.push_str(&format!("Plan: {plan}\n"));
    }
    if let Some(profiles) = duplicate_profiles {
        output.push_str(&format!(
            "Duplicate Account Identity: Account Profiles {}\n",
            profiles.join(", ")
        ));
    }

    for bucket in &view.buckets {
        output.push('\n');
        output.push_str(&format!("Limit Bucket: {}\n", bucket.label));
        for window in &bucket.windows {
            render_window(&mut output, window);
        }
    }

    output
}

fn render_window(output: &mut String, window: &QuotaWindowView) {
    output.push_str(&format!(
        "  {}: {}% used; {} min window; resets {}\n",
        window.window,
        window.used_percent,
        window.window_duration_mins,
        window
            .resets_at
            .with_timezone(&chrono::Utc)
            .format("%Y-%m-%dT%H:%M:%SZ")
    ));
}

#[derive(Debug, Error)]
enum LimitrError {
    #[error(transparent)]
    Config(#[from] config::ConfigError),
    #[error("an Account Profile observation worker stopped unexpectedly")]
    ObservationWorkerPanicked,
    #[error("the interactive monitor stopped")]
    MonitorClosed,
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
