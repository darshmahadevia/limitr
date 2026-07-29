use chrono::{DateTime, FixedOffset};
use limitr::tui::{
    LimitBucketView, LiveTrace, ProfileView, QuotaWindowView, render_view, render_view_state,
};

#[test]
fn tui_renders_local_clock_reset_instant_countdown_and_numeric_utilization() {
    let now = fixed_time("2024-11-07T09:10:00+05:30");
    let profile = ProfileView {
        label: "personal".into(),
        identity: Some("developer@example.com".into()),
        plan: Some("plus".into()),
        buckets: vec![LimitBucketView {
            label: "Codex".into(),
            windows: vec![QuotaWindowView {
                window: "Primary".into(),
                used_percent: 25.0,
                window_duration_mins: 60,
                resets_at: fixed_time("2024-11-07T10:40:00+05:30"),
                trace: LiveTrace::new(),
            }],
        }],
        error: None,
        stale_observed_at: None,
    };

    let rendered = render_view(&[profile], now, 80, 24, false, 0);

    assert!(rendered.contains("Limitr  2024-11-07 09:10:00 +05:30"));
    assert!(rendered.contains("25% used"));
    assert!(rendered.contains("resets 2024-11-07 10:40:00 +05:30 (in 1h 30m)"));
}

#[test]
fn live_traces_start_empty_and_discard_old_samples_at_the_public_bound() {
    let mut trace = LiveTrace::new();
    assert!(trace.is_empty());

    for sample in 0..40 {
        trace.push(f64::from(sample));
    }

    assert_eq!(trace.len(), 24);
    assert_eq!(trace.samples().next(), Some(16.0));
    assert_eq!(trace.samples().last(), Some(39.0));
}

#[test]
fn narrow_monochrome_rendering_keeps_numeric_meaning_and_bounds_every_line() {
    let now = fixed_time("2024-11-07T09:10:00+05:30");
    let mut trace = LiveTrace::new();
    for sample in [10.0, 40.0, 90.0] {
        trace.push(sample);
    }
    let profile = ProfileView {
        label: "a-very-long-profile-label".into(),
        identity: None,
        plan: None,
        buckets: vec![LimitBucketView {
            label: "Codex".into(),
            windows: vec![QuotaWindowView {
                window: "Primary".into(),
                used_percent: 90.0,
                window_duration_mins: 60,
                resets_at: fixed_time("2024-11-07T10:40:00+05:30"),
                trace,
            }],
        }],
        error: None,
        stale_observed_at: None,
    };

    let rendered = render_view(&[profile], now, 32, 20, true, 0);

    assert!(rendered.contains("90% used"));
    assert!(rendered.contains("Live Trace: .+#"));
    assert!(rendered.lines().all(|line| line.chars().count() <= 32));
}

#[test]
fn scrolling_multiple_profiles_keeps_the_live_clock_and_quit_help_visible() {
    let now = fixed_time("2024-11-07T09:10:00+05:30");
    let profiles = ["personal", "work", "spare"]
        .into_iter()
        .map(|label| ProfileView {
            label: label.into(),
            identity: None,
            plan: None,
            buckets: Vec::new(),
            error: None,
            stale_observed_at: None,
        })
        .collect::<Vec<_>>();

    let first = render_view(&profiles, now, 40, 6, true, 0);
    let scrolled = render_view(
        &profiles,
        fixed_time("2024-11-07T09:10:01+05:30"),
        40,
        6,
        true,
        3,
    );
    let clamped = render_view(&profiles, now, 40, 6, true, u16::MAX);
    let normalized = render_view_state(&profiles, now, 40, 6, true, u16::MAX);

    assert!(first.contains("Account Profile: personal"));
    assert!(!first.contains("Account Profile: spare"));
    assert!(scrolled.contains("Account Profile: spare"));
    assert!(clamped.contains("Account Profile: spare"));
    assert_eq!(normalized.scroll, 2);
    assert!(scrolled.starts_with("Limitr  2024-11-07 09:10:01 +05:30"));
    assert!(scrolled.ends_with("Up/Down scroll  q/Esc quit"));
    assert_ne!(first, scrolled);
}

#[test]
fn stale_snapshots_show_their_observation_age_without_hiding_limits() {
    let now = fixed_time("2024-11-07T09:10:30+05:30");
    let profile = ProfileView {
        label: "work".into(),
        identity: Some("developer@example.com".into()),
        plan: Some("plus".into()),
        buckets: vec![LimitBucketView {
            label: "Codex".into(),
            windows: vec![QuotaWindowView {
                window: "Primary".into(),
                used_percent: 25.0,
                window_duration_mins: 60,
                resets_at: fixed_time("2024-11-07T10:40:00+05:30"),
                trace: LiveTrace::new(),
            }],
        }],
        error: Some("connection lost; retrying".into()),
        stale_observed_at: Some(fixed_time("2024-11-07T09:10:00+05:30")),
    };

    let rendered = render_view(&[profile], now, 80, 24, true, 0);

    assert!(rendered.contains("Stale Snapshot: observed 30s ago"));
    assert!(rendered.contains("25% used"));
    assert!(rendered.contains("Error: connection lost; retrying"));
}

fn fixed_time(value: &str) -> DateTime<FixedOffset> {
    DateTime::parse_from_rfc3339(value).expect("valid fixed time")
}
