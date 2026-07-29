use std::collections::{BTreeMap, VecDeque};

use chrono::{DateTime, FixedOffset};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

const LIVE_TRACE_CAPACITY: usize = 24;

#[derive(Clone, Debug, Default)]
pub struct LiveTrace {
    samples: VecDeque<f64>,
}

impl LiveTrace {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, used_percent: f64) {
        if self.samples.len() == LIVE_TRACE_CAPACITY {
            self.samples.pop_front();
        }
        self.samples.push_back(used_percent);
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn samples(&self) -> impl DoubleEndedIterator<Item = f64> + '_ {
        self.samples.iter().copied()
    }
}

#[derive(Clone, Debug)]
pub struct QuotaWindowView {
    pub window: String,
    pub used_percent: f64,
    pub window_duration_mins: u64,
    pub resets_at: DateTime<FixedOffset>,
    pub trace: LiveTrace,
}

#[derive(Clone, Debug)]
pub struct LimitBucketView {
    pub label: String,
    pub windows: Vec<QuotaWindowView>,
}

#[derive(Clone, Debug)]
pub struct ProfileView {
    pub label: String,
    pub identity: Option<String>,
    pub plan: Option<String>,
    pub buckets: Vec<LimitBucketView>,
    pub error: Option<String>,
    pub stale_observed_at: Option<DateTime<FixedOffset>>,
}

#[derive(Debug)]
pub struct RenderedView {
    pub text: String,
    pub scroll: u16,
}

pub fn duplicate_account_identity_profiles<'a>(
    profiles: impl IntoIterator<Item = (&'a str, Option<&'a str>)>,
) -> BTreeMap<String, Vec<String>> {
    let mut identity_profiles = BTreeMap::new();
    for (label, identity) in profiles {
        if let Some(identity) = identity {
            identity_profiles
                .entry(identity.to_owned())
                .or_insert_with(Vec::new)
                .push(label.to_owned());
        }
    }
    identity_profiles.retain(|_, profiles| profiles.len() > 1);
    identity_profiles
}

pub fn duplicate_account_identity_line(profiles: &[String]) -> String {
    format!(
        "Duplicate Account Identity: Account Profiles {}",
        profiles.join(", ")
    )
}

pub fn render_view(
    profiles: &[ProfileView],
    now: DateTime<FixedOffset>,
    width: u16,
    height: u16,
    ascii: bool,
    scroll: u16,
) -> String {
    render_view_state(profiles, now, width, height, ascii, scroll).text
}

pub fn render_view_state(
    profiles: &[ProfileView],
    now: DateTime<FixedOffset>,
    width: u16,
    height: u16,
    ascii: bool,
    scroll: u16,
) -> RenderedView {
    let account_profile_word = if profiles.len() == 1 {
        "Account Profile"
    } else {
        "Account Profiles"
    };
    let separator = if ascii { "|" } else { "•" };
    let header = format!(
        "Limitr  {}  {separator}  {} {account_profile_word}",
        now.format("%Y-%m-%d %H:%M:%S %:z"),
        profiles.len()
    );
    let identity_profiles = duplicate_account_identity_profiles(
        profiles
            .iter()
            .map(|profile| (profile.label.as_str(), profile.identity.as_deref())),
    );
    let mut lines = if profiles.is_empty() {
        vec![
            String::new(),
            "No Account Profiles configured.".into(),
            "Add one with: limitr profile add <label> <absolute Codex Home>".into(),
        ]
    } else {
        Vec::new()
    };
    for profile in profiles {
        lines.push(String::new());
        lines.push(format!("Account Profile: {}", profile.label));
        if let Some(identity) = &profile.identity {
            lines.push(format!("Account Identity: {identity}"));
            if let Some(duplicates) = identity_profiles.get(identity.as_str()) {
                lines.push(duplicate_account_identity_line(duplicates));
            }
        }
        if let Some(plan) = &profile.plan {
            lines.push(format!("Plan: {plan}"));
        }
        if profile.identity.is_none() && profile.buckets.is_empty() && profile.error.is_none() {
            lines.push("Connecting to Codex...".into());
        }
        if let Some(observed_at) = profile.stale_observed_at {
            let age = now.signed_duration_since(observed_at).num_seconds().max(0);
            lines.push(format!("Stale Snapshot: observed {} ago", format_age(age)));
        }
        for bucket in &profile.buckets {
            lines.push(format!("Limit Bucket: {}", bucket.label));
            for window in &bucket.windows {
                let remaining = window.resets_at.signed_duration_since(now);
                lines.push(format!(
                    "{}  |  {} min window",
                    window.window, window.window_duration_mins
                ));
                lines.push(format!(
                    "{}% used  {}  Live Trace: {}",
                    format_percent(window.used_percent),
                    utilization_bar(window.used_percent, width, ascii),
                    render_trace(&window.trace, ascii)
                ));
                lines.push(format!(
                    "resets {} ({})",
                    window.resets_at.format("%Y-%m-%d %H:%M:%S %:z"),
                    format_countdown(remaining.num_seconds())
                ));
            }
        }
        if let Some(error) = &profile.error {
            lines.push(format!("Error: {error}"));
        }
    }
    let mut visible = wrap_lines(vec![header], width);
    let body = wrap_lines(lines, width);
    let controls = if ascii {
        "j/k scroll  PgUp/PgDn page  Home/End jump  q quit"
    } else {
        "↑/↓ scroll  PgUp/PgDn page  Home/End jump  q quit"
    };
    let largest_line = body.len();
    let reserved_footer = wrap_lines(
        vec![format!(
            "{controls}  Lines {largest_line}-{largest_line}/{largest_line}"
        )],
        width,
    );
    let body_height = usize::from(height).saturating_sub(visible.len() + reserved_footer.len());
    let effective_scroll = usize::from(scroll).min(body.len().saturating_sub(body_height));
    let first_line = if body.is_empty() {
        0
    } else {
        effective_scroll + 1
    };
    let last_line = (effective_scroll + body_height).min(body.len());
    let mut footer = wrap_lines(
        vec![format!(
            "{controls}  Lines {first_line}-{last_line}/{}",
            body.len()
        )],
        width,
    );
    while footer.len() < reserved_footer.len() {
        footer.insert(0, String::new());
    }
    visible.extend(body.into_iter().skip(effective_scroll).take(body_height));
    visible.extend(footer);
    RenderedView {
        text: visible.join("\n"),
        scroll: effective_scroll as u16,
    }
}

fn format_age(seconds: i64) -> String {
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3_600 {
        format!("{}m {}s", seconds / 60, seconds % 60)
    } else {
        format!("{}h {}m", seconds / 3_600, seconds % 3_600 / 60)
    }
}

fn format_countdown(seconds: i64) -> String {
    if seconds <= 0 {
        return "now".into();
    }
    let hours = seconds / 3600;
    let minutes = seconds % 3600 / 60;
    if hours > 0 {
        format!("in {hours}h {minutes}m")
    } else {
        format!("in {minutes}m")
    }
}

fn format_percent(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        format!("{value:.1}")
    }
}

fn utilization_bar(used_percent: f64, width: u16, ascii: bool) -> String {
    let bar_width = usize::from(width.saturating_sub(35).clamp(4, 12));
    let filled = ((used_percent.clamp(0.0, 100.0) / 100.0) * bar_width as f64).round() as usize;
    let (full, empty) = if ascii { ('#', '-') } else { ('█', '░') };
    format!(
        "[{}{}]",
        full.to_string().repeat(filled),
        empty.to_string().repeat(bar_width - filled)
    )
}

fn render_trace(trace: &LiveTrace, ascii: bool) -> String {
    if trace.is_empty() {
        return if ascii {
            "(collecting)".into()
        } else {
            "…".into()
        };
    }
    trace
        .samples()
        .map(|sample| {
            if ascii {
                match sample {
                    value if value < 34.0 => '.',
                    value if value < 67.0 => '+',
                    _ => '#',
                }
            } else {
                const LEVELS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
                let index = ((sample.clamp(0.0, 100.0) / 100.0) * 7.0).round() as usize;
                LEVELS[index]
            }
        })
        .collect()
}

fn wrap_lines(lines: Vec<String>, width: u16) -> Vec<String> {
    let width = usize::from(width.max(1));
    let mut wrapped = Vec::new();
    for line in lines {
        if line.is_empty() {
            wrapped.push(String::new());
            continue;
        }
        let mut remaining = line.as_str();
        while !remaining.is_empty() {
            if UnicodeWidthStr::width(remaining) <= width {
                wrapped.push(remaining.to_owned());
                break;
            }
            let mut display_width = 0;
            let mut hard_end = 0;
            let mut word_end = None;
            for (index, grapheme) in remaining.grapheme_indices(true) {
                let grapheme_width = UnicodeWidthStr::width(grapheme);
                if display_width + grapheme_width > width {
                    break;
                }
                display_width += grapheme_width;
                hard_end = index + grapheme.len();
                if index > 0 && grapheme.chars().all(char::is_whitespace) {
                    word_end = Some(index);
                }
            }
            if hard_end == 0 {
                let grapheme = remaining
                    .graphemes(true)
                    .next()
                    .expect("remaining text is non-empty");
                wrapped.push("…".into());
                remaining = &remaining[grapheme.len()..];
            } else {
                let end = word_end.unwrap_or(hard_end);
                wrapped.push(remaining[..end].trim_end().to_owned());
                remaining = remaining[end..].trim_start();
            }
        }
    }
    wrapped
}
