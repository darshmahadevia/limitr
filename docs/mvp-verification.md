# MVP verification review

This review maps the user stories and acceptance boundaries in parent
specification [#1](https://github.com/darshmahadevia/limitr/issues/1) to the
executable at the completion of issue
[#6](https://github.com/darshmahadevia/limitr/issues/6). It is a review record,
not a second product specification; `CONTEXT.md` and the ADRs remain normative.

## User stories

| # | Executable evidence | Review |
| --- | --- | --- |
| 1 | `load_account_profiles` synthesizes a non-persisted `default` profile from `CODEX_HOME` or the normal Codex Home. `tests/status.rs` covers both paths. | Met |
| 2–4 | `limitr profile add/list/remove` persists labels and absolute Codex Home paths without modifying the homes. `tests/profiles.rs` covers commands and invariants. | Met |
| 5–6 | Status and TUI view models render the configured label and optional Codex-reported email and plan. | Met |
| 7 | Status and the shared TUI renderer flag every profile sharing a reported email without collapsing profiles. `tests/multiple_status.rs` and `tests/tui_view.rs` cover both surfaces. | Met |
| 8–11 | The renderer prefers all `rateLimitsByLimitId` buckets, falls back to the legacy bucket, and shows each window's numeric percent and utilization bar. `tests/status.rs` and `tests/tui_view.rs` cover this. | Met |
| 12–13 | Each TUI Quota Window has a bounded 24-sample Live Trace held by the process. `tests/tui_view.rs` covers the bound; no snapshot or trace serialization exists. | Met |
| 14–16 | The TUI refreshes a local wall clock, renders absolute local Reset Instants and derives countdowns from them on each render. Fixed-time view tests cover the output. | Met |
| 17–18 | `monitor_profile` refetches on `account/rateLimits/updated` and every 30 seconds by default. Interactive fake-server tests cover notification and periodic reconciliation. | Met |
| 19–20 | Each profile has an independent monitor worker; failures retain and age the last successful in-memory snapshot while retrying. Resilience and TUI tests cover isolation, staleness, recovery, and backoff. | Met |
| 21–23 | Unauthenticated and unsupported authentication states are actionable; missing methods/results/fields produce sanitized profile-scoped compatibility errors. `tests/resilience.rs` covers these cases. | Met |
| 24–25 | `Command::new("codex")` uses normal `PATH` resolution. Tests and nonstandard installs override the executable by prepending to `PATH`. | Met |
| 26–27 | `limitr status` takes one concurrent observation per profile, includes successes and failures, then returns 0, 1, or 2 as documented. | Met |
| 28–29 | The text-first TUI wraps, scrolls, preserves the header/footer, and has a complete numeric ASCII mode selected by `NO_COLOR` or `TERM=dumb`. View tests cover narrow and monochrome output. | Met |
| 30 | RAII cleanup kills and waits only for the spawned app-server child; terminal and interactive integration tests cover clean shutdown and child reaping. | Met |
| 31–33 | Integration is exclusively through app-server account read methods; child stderr and server error text are not surfaced; no credential, mutation, logout, or reset-credit code path exists. Sanitization tests use credential-shaped fabricated data. | Met |
| 34 | Integration tests create local fake newline-delimited JSON-RPC app-servers for snapshots, multiple profiles, compatibility, notifications, failures, timeouts, recovery, and cleanup. | Met |
| 35 | `.github/workflows/ci.yml` runs formatting, compilation, strict Clippy, and all targets on Linux and macOS. | Met |
| 36 | `README.md` documents prerequisites, source installation, first run, commands, TUI controls, privacy, compatibility, troubleshooting, and contributor verification. | Met |

## Acceptance boundaries

| Boundary | Executable evidence |
| --- | --- |
| Present-tense, read-only monitoring | Only `initialize`, `account/read`, and `account/rateLimits/read` requests plus the `initialized` notification are sent. No mutation or reset-credit method exists. |
| Local profile vs. remote identity | Configuration stores an Account Profile label and Codex Home; identity email and plan come only from `account/read` and are display data. |
| No persisted observations | The only write path serializes `Config { profiles }` to TOML. Snapshots, stale state, and traces are process-memory types. |
| No direct credential access | Limitr passes a Codex Home as `CODEX_HOME` to its child and never opens files within it. Fake-server tests place credential markers in Codex Homes and verify they remain untouched. |
| Compatibility fails truthfully | Required methods and fields are validated. Unknown response fields deserialize permissively, while missing required behavior returns an actionable incompatibility error without a scraping or credential-parsing fallback. |
| Authentication boundary | ChatGPT identities are observed; unauthenticated, API-key, Bedrock, and unknown non-ChatGPT authentication are not converted into fabricated percentages. |
| Failure isolation and cleanup | Profile observations run independently. Interactive retries are bounded, stale data is labelled, and owned child processes are killed and waited on during shutdown. |
| Human-facing MVP output | Status is one-shot human-readable output with no promised machine-readable schema. TUI meaning remains present in ASCII and narrow layouts. |
| Test isolation | Automated tests use fabricated local app-server fixtures and temporary configuration roots, with no real credential or network access. |
| Supported verification platforms | CI continuously verifies current Linux and macOS runners. Windows paths exist in the implementation but Windows is not an MVP CI guarantee. |

## Release gate

From a clean clone with stable Rust and Codex prerequisites installed:

```sh
cargo fmt --check
cargo check --all-targets --all-features
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets
cargo install --path .
limitr --version
```
