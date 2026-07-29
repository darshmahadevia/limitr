use chrono::{DateTime, FixedOffset};
use limitr::tui::{
    LimitBucketView, LiveTrace, ProfileView, QuotaWindowView, render_view, render_view_state,
};
use unicode_width::UnicodeWidthStr;

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
    assert!(rendered.contains("60 min window"));
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
    assert!(rendered.contains("Live Trace:"));
    assert!(rendered.contains(".+#"));
    assert!(rendered.lines().all(|line| line.chars().count() <= 32));
}

#[test]
fn unicode_identity_text_never_exceeds_the_terminal_width() {
    let now = fixed_time("2024-11-07T09:10:00+05:30");
    let profile = ProfileView {
        label: "仕事用アカウント🧑🏽‍💻".into(),
        identity: Some("開発者🧪@example.test".into()),
        plan: Some("チーム".into()),
        buckets: Vec::new(),
        error: None,
        stale_observed_at: None,
    };

    let rendered = render_view(&[profile], now, 20, 20, false, 0);

    assert!(
        rendered
            .lines()
            .all(|line| UnicodeWidthStr::width(line) <= 20),
        "rendered line exceeded terminal width:\n{rendered}"
    );
}

#[test]
fn indivisible_wide_graphemes_use_an_explicit_tiny_terminal_placeholder() {
    let profile = ProfileView {
        label: "界".into(),
        identity: Some("🧪".into()),
        plan: None,
        buckets: Vec::new(),
        error: None,
        stale_observed_at: None,
    };

    let rendered = render_view(
        &[profile],
        fixed_time("2024-11-07T09:10:00+05:30"),
        1,
        200,
        false,
        0,
    );

    assert!(rendered.contains('…'));
    assert!(!rendered.contains('?'));
    assert!(
        rendered
            .lines()
            .all(|line| UnicodeWidthStr::width(line) <= 1)
    );
}

#[test]
fn account_profile_without_a_limit_snapshot_has_an_explicit_loading_state() {
    let profile = ProfileView {
        label: "work".into(),
        identity: None,
        plan: None,
        buckets: Vec::new(),
        error: None,
        stale_observed_at: None,
    };

    let rendered = render_view(
        &[profile],
        fixed_time("2024-11-07T09:10:00+05:30"),
        80,
        24,
        true,
        0,
    );

    assert!(rendered.contains("Connecting to Codex..."));
}

#[test]
fn empty_configuration_explains_how_to_add_an_account_profile() {
    let rendered = render_view(
        &[],
        fixed_time("2024-11-07T09:10:00+05:30"),
        80,
        24,
        true,
        0,
    );

    assert!(rendered.contains("No Account Profiles configured."));
    assert!(rendered.contains("limitr profile add"));
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
        u16::MAX,
    );
    let clamped = render_view(&profiles, now, 40, 6, true, u16::MAX);
    let normalized = render_view_state(&profiles, now, 40, 6, true, u16::MAX);

    assert!(first.contains("Account Profile: personal"));
    assert!(!first.contains("Account Profile: spare"));
    assert!(scrolled.contains("Account Profile: spare"));
    assert!(clamped.contains("Account Profile: spare"));
    assert!(normalized.scroll > 0);
    assert!(scrolled.starts_with("Limitr  2024-11-07 09:10:01 +05:30"));
    assert!(first.contains("Lines "));
    assert!(scrolled.contains("Lines "));
    assert!(
        scrolled.contains("Home/End") && scrolled.contains("jump"),
        "missing jump controls:\n{scrolled}"
    );
    assert_ne!(first, scrolled);
}

#[test]
fn footer_range_matches_the_number_of_visible_body_lines_at_the_end() {
    let profiles = (0..100)
        .map(|index| ProfileView {
            label: format!("account-{index:03}"),
            identity: None,
            plan: None,
            buckets: Vec::new(),
            error: None,
            stale_observed_at: None,
        })
        .collect::<Vec<_>>();

    for width in 20..=60 {
        let top = render_view(
            &profiles,
            fixed_time("2024-11-07T09:10:00+05:30"),
            width,
            600,
            true,
            0,
        );
        let body_start = top
            .lines()
            .position(str::is_empty)
            .expect("top-of-view body separator");
        let rendered = render_view(
            &profiles,
            fixed_time("2024-11-07T09:10:00+05:30"),
            width,
            12,
            true,
            u16::MAX,
        );
        let lines = rendered.lines().collect::<Vec<_>>();
        let footer_start = lines
            .iter()
            .position(|line| line.starts_with("j/k"))
            .expect("footer");
        let footer = lines[footer_start..].join(" ");
        let range = footer
            .split("Lines ")
            .nth(1)
            .expect("line range")
            .split_whitespace()
            .next()
            .expect("range value");
        let (visible_range, total) = range.split_once('/').expect("range total");
        let (first, last) = visible_range.split_once('-').expect("range endpoints");
        let first: usize = first.parse().expect("first line");
        let last: usize = last.parse().expect("last line");
        let total: usize = total.parse().expect("total lines");
        let visible_body_lines = footer_start - body_start;

        assert_eq!(last, total, "width {width}:\n{rendered}");
        assert_eq!(
            first,
            total - visible_body_lines + 1,
            "width {width}:\n{rendered}"
        );
    }
}

#[test]
fn duplicate_account_identities_are_flagged_without_collapsing_tui_profiles() {
    let now = fixed_time("2024-11-07T09:10:00+05:30");
    let profiles = ["personal", "work"]
        .into_iter()
        .map(|label| ProfileView {
            label: label.into(),
            identity: Some("shared@example.test".into()),
            plan: Some("plus".into()),
            buckets: Vec::new(),
            error: None,
            stale_observed_at: None,
        })
        .collect::<Vec<_>>();

    let rendered = render_view(&profiles, now, 80, 24, true, 0);

    assert!(rendered.contains("Account Profile: personal"));
    assert!(rendered.contains("Account Profile: work"));
    assert_eq!(
        rendered
            .matches("Duplicate Account Identity: Account Profiles personal, work")
            .count(),
        2
    );
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

#[test]
fn many_account_profiles_and_quota_windows_remain_bounded_on_a_wide_terminal() {
    let profiles = (0..500)
        .map(|index| ProfileView {
            label: format!("account-{index:03}"),
            identity: Some(format!("developer-{index:03}@example.test")),
            plan: Some("plus".into()),
            buckets: vec![LimitBucketView {
                label: "Codex".into(),
                windows: ["Primary", "Secondary"]
                    .into_iter()
                    .map(|window| QuotaWindowView {
                        window: window.into(),
                        used_percent: 75.0,
                        window_duration_mins: 10_080,
                        resets_at: fixed_time("2030-01-02T03:04:05+05:30"),
                        trace: LiveTrace::new(),
                    })
                    .collect(),
            }],
            error: None,
            stale_observed_at: None,
        })
        .collect::<Vec<_>>();

    let rendered = render_view(
        &profiles,
        fixed_time("2024-11-07T09:10:00+05:30"),
        240,
        60,
        false,
        u16::MAX,
    );

    assert!(rendered.contains("Account Profile: account-499"));
    assert!(rendered.contains("Primary  |  10080 min window"));
    assert!(rendered.contains("Secondary  |  10080 min window"));
    assert!(rendered.contains("Lines "));
    assert!(
        rendered
            .lines()
            .all(|line| UnicodeWidthStr::width(line) <= 240)
    );
}

fn fixed_time(value: &str) -> DateTime<FixedOffset> {
    DateTime::parse_from_rfc3339(value).expect("valid fixed time")
}
