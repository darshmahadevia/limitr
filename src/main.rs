mod app_server;
mod config;
mod monitor;

use std::collections::{HashMap, HashSet};
use std::env;
use std::io::{Stdout, stdout};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use app_server::AppServerError;
use chrono::{DateTime, Local};
use clap::{Parser, Subcommand};
use config::{AccountProfile, Config, config_file};
use crossterm::cursor::{Hide, Show};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use limitr::tui::{
    LimitBucketView, LiveTrace, ProfileView, QuotaWindowView, duplicate_account_identity_line,
    duplicate_account_identity_profiles, render_view_state,
};
use monitor::{
    LimitBucket, LimitSnapshot, MonitorEvent, MonitorPolicy, RateLimitsReadResult, monitor_profile,
    observe_profile,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use thiserror::Error;

#[derive(Debug, Parser)]
#[command(name = "limitr", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
    /// Milliseconds between fallback rate-limit reconciliation reads.
    #[arg(long, default_value_t = 30_000, hide = true)]
    reconcile_interval_ms: u64,
    /// Milliseconds to wait for each app-server response.
    #[arg(long, default_value_t = 10_000, hide = true)]
    request_timeout_ms: u64,
    /// Initial milliseconds between recovery attempts.
    #[arg(long, default_value_t = 250, hide = true)]
    retry_initial_ms: u64,
    /// Maximum milliseconds between recovery attempts.
    #[arg(long, default_value_t = 5_000, hide = true)]
    retry_max_ms: u64,
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
    let policy = MonitorPolicy {
        response_timeout: Duration::from_millis(cli.request_timeout_ms.max(1)),
        retry_initial: Duration::from_millis(cli.retry_initial_ms.max(1)),
        retry_max: Duration::from_millis(cli.retry_max_ms.max(cli.retry_initial_ms).max(1)),
    };
    match cli.command {
        Some(Commands::Status) => print_status(policy.response_timeout),
        Some(Commands::Profile { command }) => {
            manage_profiles(command)?;
            Ok(0)
        }
        None => {
            run_interactive(
                Duration::from_millis(cli.reconcile_interval_ms.max(1)),
                policy,
            )?;
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

fn run_interactive(reconcile_interval: Duration, policy: MonitorPolicy) -> Result<(), LimitrError> {
    let profiles = load_account_profiles()?.profiles;
    let mut monitored: Vec<_> = profiles
        .into_iter()
        .map(|profile| MonitoredProfile::start(profile, reconcile_interval, policy))
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
        let mut should_quit = false;
        for _ in 0..256 {
            if let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
            {
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => should_quit = true,
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        should_quit = true;
                    }
                    KeyCode::Up | KeyCode::Char('k') => scroll = scroll.saturating_sub(1),
                    KeyCode::Down | KeyCode::Char('j') => scroll = scroll.saturating_add(1),
                    KeyCode::PageUp => scroll = scroll.saturating_sub(5),
                    KeyCode::PageDown => scroll = scroll.saturating_add(5),
                    KeyCode::Home | KeyCode::Char('g') => scroll = 0,
                    KeyCode::End | KeyCode::Char('G') => scroll = u16::MAX,
                    _ => {}
                }
            }
            if should_quit || !event::poll(Duration::ZERO)? {
                break;
            }
        }
        if should_quit {
            break;
        }
    }
    Ok(())
}

struct LoadedProfiles {
    profiles: Vec<AccountProfile>,
}

fn load_account_profiles() -> Result<LoadedProfiles, LimitrError> {
    Ok(match Config::load(&config_file()?)? {
        Some(config) => LoadedProfiles {
            profiles: config.into_profiles(),
        },
        None => LoadedProfiles {
            profiles: vec![AccountProfile {
                label: "default".into(),
                codex_home: default_codex_home()?,
            }],
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
            let border_style = if ascii {
                Style::default()
            } else {
                Style::default().fg(Color::Cyan)
            };
            let paragraph = Paragraph::new(rendered.text.as_str())
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_style(border_style)
                        .title(" Limitr "),
                )
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
    last_observed_at: Option<DateTime<chrono::FixedOffset>>,
}

impl MonitoredProfile {
    fn start(profile: AccountProfile, reconcile_interval: Duration, policy: MonitorPolicy) -> Self {
        let (event_sender, events) = mpsc::channel();
        let (stop, stop_receiver) = mpsc::channel();
        let label = profile.label.clone();
        let worker = thread::spawn(move || {
            monitor_profile(
                profile.codex_home,
                reconcile_interval,
                policy,
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
                stale_observed_at: None,
            },
            traces: HashMap::new(),
            events,
            stop,
            worker: Some(worker),
            last_observed_at: None,
        }
    }

    fn receive_events(&mut self) {
        loop {
            match self.events.try_recv() {
                Ok(MonitorEvent::Snapshot(snapshot)) => self.apply_snapshot(snapshot),
                Ok(MonitorEvent::Error(error)) => {
                    if self.view.buckets.is_empty() {
                        self.view.stale_observed_at = None;
                    } else {
                        self.view.stale_observed_at = self.last_observed_at;
                    }
                    self.view.error = Some(error);
                }
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
    }

    fn apply_snapshot(&mut self, snapshot: LimitSnapshot) {
        let has_identity_observation = snapshot.identity.is_some();
        let identity = self.view.identity.take();
        let plan = self.view.plan.take();
        match profile_view(self.view.label.clone(), snapshot, &mut self.traces) {
            Ok(mut view) => {
                if !has_identity_observation {
                    view.identity = identity;
                    view.plan = plan;
                }
                let observed_at = Local::now().fixed_offset();
                view.stale_observed_at = None;
                self.last_observed_at = Some(observed_at);
                self.view = view;
            }
            Err(error) => {
                self.view.identity = identity;
                self.view.plan = plan;
                if !self.view.buckets.is_empty() {
                    self.view.stale_observed_at = self.last_observed_at;
                }
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
        stale_observed_at: None,
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

fn print_status(response_timeout: Duration) -> Result<i32, LimitrError> {
    let LoadedProfiles { profiles } = load_account_profiles()?;

    let observations: Vec<_> = profiles
        .into_iter()
        .map(|profile| {
            thread::spawn(move || {
                let result = observe_profile(&profile.codex_home, response_timeout);
                (profile.label, result)
            })
        })
        .collect();
    let observations = observations
        .into_iter()
        .map(|observation| {
            let (label, result) = observation
                .join()
                .map_err(|_| LimitrError::ObservationWorkerPanicked)?;
            Ok((label, result))
        })
        .collect::<Result<Vec<_>, LimitrError>>()?;
    let identity_profiles =
        duplicate_account_identity_profiles(observations.iter().map(|(label, observation)| {
            let identity = observation
                .as_ref()
                .ok()
                .and_then(|snapshot| snapshot.identity.as_ref())
                .and_then(|identity| identity.email.as_deref());
            (label.as_str(), identity)
        }));
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
                    .and_then(|identity| identity.email.as_deref())
                    .and_then(|identity| identity_profiles.get(identity))
                    .map(Vec::as_slice);
                let mut traces = HashMap::new();
                let view = profile_view(label, snapshot, &mut traces)?;
                Ok::<_, LimitrError>(render_status(&view, duplicates))
            }
            Err(error) => Ok(render_profile_error(&label, &error)),
        })
        .collect::<Result<Vec<_>, _>>()?;
    print!("{}", rendered.join("\n"));
    Ok(if failure_count == 0 { 0 } else { 2 })
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

fn render_status(view: &ProfileView, duplicate_profiles: Option<&[String]>) -> String {
    let mut output = format!("Account Profile: {}\n", view.label);
    if let Some(identity) = &view.identity {
        output.push_str(&format!("Account Identity: {identity}\n"));
    }
    if let Some(plan) = &view.plan {
        output.push_str(&format!("Plan: {plan}\n"));
    }
    if let Some(profiles) = duplicate_profiles {
        output.push_str(&duplicate_account_identity_line(profiles));
        output.push('\n');
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

fn render_profile_error(label: &str, error: &LimitrError) -> String {
    let detail = match error {
        LimitrError::Unauthenticated => {
            "Unauthenticated: sign in to this Codex Home with Codex, then retry".into()
        }
        LimitrError::UnsupportedAuthentication(kind) => {
            format!("Unsupported: {kind} authentication does not provide ChatGPT rate limits")
        }
        _ => format!("Error: {error}"),
    };
    format!("Account Profile: {label}\n{detail}\n")
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
    #[error("the Account Profile is unauthenticated; sign in to this Codex Home with Codex")]
    Unauthenticated,
    #[error("{0} authentication does not provide ChatGPT rate limits")]
    UnsupportedAuthentication(&'static str),
    #[error("could not determine the normal Codex Home; set CODEX_HOME")]
    CodexHomeUnavailable,
    #[error(transparent)]
    AppServer(#[from] AppServerError),
    #[error("Codex app-server returned an invalid Reset Instant: {0}")]
    InvalidResetInstant(i64),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl LimitrError {
    fn is_retryable(&self) -> bool {
        !matches!(
            self,
            Self::Unauthenticated
                | Self::UnsupportedAuthentication(_)
                | Self::InvalidResetInstant(_)
                | Self::CodexHomeUnavailable
                | Self::Config(_)
        ) && match self {
            Self::AppServer(error) => error.is_retryable(),
            _ => true,
        }
    }
}
