# Limitr

Limitr is a local-first, read-only CLI/TUI for viewing current Codex rate
limits across multiple Account Profiles. An Account Profile is a locally named
connection to one Codex Home; the Account Identity shown for it is reported by
Codex.

Limitr shows every reported Limit Bucket and its primary and secondary Quota
Windows, including numeric utilization and Reset Instants. The interactive view
also keeps a compact Live Trace for each window for as long as that process is
running.

## Prerequisites

- Linux or macOS. The implementation has Windows-aware paths, but the MVP is
  continuously tested only on Linux and macOS.
- [Rust](https://www.rust-lang.org/tools/install) 1.85 or newer, including
  Cargo. Limitr uses Rust 2024 edition.
- A `codex` executable available through `PATH`.
- At least one Codex Home signed in with ChatGPT authentication. API-key and
  Amazon Bedrock authentication do not expose comparable ChatGPT rate limits.

Limitr requires a Codex app-server that supports the standard initialize
handshake, `account/read`, and `account/rateLimits/read`. It also listens for
`account/rateLimits/updated` in the interactive view. See
[Compatibility](#compatibility) for details.

## Install from source

Clone the repository, build and test it, then install the binary with Cargo:

```sh
git clone https://github.com/darshmahadevia/limitr.git
cd limitr
cargo test --all-targets
cargo install --path .
limitr --version
```

`cargo install --path .` places `limitr` in Cargo's binary directory, normally
`$HOME/.cargo/bin`. Add that directory to `PATH` if the final command is not
found. To run without installing, replace `limitr` in the examples with
`cargo run --`.

## First run and default behavior

Run the default interactive view:

```sh
limitr
```

When no Limitr configuration file exists, Limitr synthesizes a non-persisted
Account Profile labelled `default`. Its Codex Home is `CODEX_HOME` when that
environment variable is set and non-empty; otherwise it is the normal Codex
Home (`$HOME/.codex` on Linux and macOS). This first run does not create Limitr
configuration.

Use the one-shot command when an interactive terminal is not wanted:

```sh
limitr status
```

`status` starts one Codex app-server child per Account Profile, observes one
Limit Snapshot for each profile concurrently, prints profiles in configured
order, and exits. If a configuration file exists but contains no profiles,
Limitr observes none; it does not recreate the synthetic default.

The following example is entirely fabricated. The identities, percentages,
bucket names, and timestamps do not represent real accounts:

```text
Account Profile: personal
Account Identity: alex@example.test
Plan: plus

Limit Bucket: Codex (codex)
  Primary: 24% used; 300 min window; resets 2030-01-02T03:04:05Z
  Secondary: 61.5% used; 10080 min window; resets 2030-01-08T03:04:05Z

Account Profile: studio
Account Identity: robin@example.test
Plan: team

Limit Bucket: codex_other
  Primary: 8% used; 60 min window; resets 2030-01-02T04:00:00Z
```

## Account Profile management

Register an Account Profile with a unique label and an absolute Codex Home:

```sh
limitr profile add personal /Users/example/.codex-personal
limitr profile add work /Users/example/.codex-work
limitr profile list
limitr profile remove work
```

The paths above are fabricated. On Linux they might instead look like
`/home/example/.codex-personal`.

Labels and Codex Homes must each be unique. `profile list` prints the configured
label and path in configuration order. `profile remove` removes only Limitr's
metadata; it never removes or modifies the Codex Home. Once a configuration
file exists, only its profiles are monitored.

Limitr stores profile metadata in `limitr/config.toml` under the
platform-appropriate configuration directory:

- Linux: `${XDG_CONFIG_HOME:-$HOME/.config}/limitr/config.toml`
- macOS: `$XDG_CONFIG_HOME/limitr/config.toml` when `XDG_CONFIG_HOME` is set;
  otherwise
  `$HOME/Library/Application Support/limitr/config.toml`

The file is human-readable TOML, but the commands above preserve its invariants
for you.

## Status exit codes

`limitr status` uses these exit codes:

- `0`: every configured Account Profile was observed successfully, including
  the valid case where an existing configuration contains no profiles.
- `1`: Limitr could not run the command, for example because configuration is
  invalid or the configuration directory cannot be determined.
- `2`: a one-shot result was produced, but one or more Account Profiles failed.
  Successful and failed profiles are both printed.

The `status` output is intended for people and does not yet have a stable
machine-readable contract.

## Interactive TUI

The header shows the current local time. Each profile card shows its configured
label and, when reported, its email and plan. If multiple Account Profiles
report the same Account Identity, every affected card names the duplicate
profiles without merging them. Each Quota Window includes a numeric used
percentage, a utilization bar, a bounded Live Trace, and the server's absolute
Reset Instant rendered in local time with a derived countdown.

Codex rate-limit notifications trigger a refetch, and a fallback reconciliation
runs every 30 seconds. A profile failure is isolated from other profiles. After
a previously successful profile fails, its last in-memory Limit Snapshot
remains visible and is marked stale with its observation age while Limitr
retries with bounded backoff.

Controls:

- `Up` / `Down` or `k` / `j`: scroll one line.
- `Page Up` / `Page Down`: scroll five lines.
- `q`, `Esc`, or `Ctrl-C`: quit.

The view wraps in narrow terminals and scrolls when there are more profiles
than fit. Set `NO_COLOR` to any value, or use `TERM=dumb`, to force the complete
ASCII/monochrome representation. Numeric values carry the meaning independently
of glyphs or color.

On clean shutdown, Limitr restores the terminal and stops only the app-server
children it started.

## Privacy and persistence

Limitr persists only Account Profile labels and absolute Codex Home paths in
its configuration file. The current MVP has no other display preferences.

Limitr does **not**:

- read, copy, decrypt, edit, or delete `auth.json`, keychain entries, or any
  other credentials;
- persist Limit Snapshots, Stale Snapshots, Live Traces, token activity, or
  other usage observations;
- create a usage database or cross-launch history;
- call account mutation, logout, or reset-credit consumption methods; or
- contact private endpoints or scrape the Codex terminal.

Authentication remains Codex's responsibility. Limitr launches
`codex app-server --stdio` with the selected `CODEX_HOME` and uses only its
structured account and rate-limit methods. App-server stderr is suppressed, and
protocol errors are converted to sanitized, actionable diagnostics rather than
echoing server-provided text. The normal display intentionally shows the
Codex-reported email and plan when available.

Live Traces contain at most 24 samples per Quota Window and exist only in
process memory. They and the latest Limit Snapshots disappear when Limitr exits.

## Compatibility

There is no hard-coded Codex version check because compatibility is defined by
behavior. The minimum compatible `codex app-server --stdio` must:

1. complete the initialize/initialized handshake;
2. provide `account/read` with `account` and `requiresOpenaiAuth`;
3. provide `account/rateLimits/read` with the legacy `rateLimits` object and,
   optionally, `rateLimitsByLimitId`; and
4. provide `usedPercent`, `windowDurationMins`, and `resetsAt` for each present
   primary or secondary Quota Window.

Limitr prefers a non-empty `rateLimitsByLimitId` map so newly introduced Limit
Buckets are not hidden, and falls back to `rateLimits`. Unknown fields are
ignored for forward compatibility. A missing method, result, or required field
is reported as a profile-scoped `incompatible Codex app-server` error that names
the method or field and tells you to update Codex. No credential parsing or
screen-scraping fallback is attempted.

Normal command resolution chooses the Codex executable. For development,
testing, or a nonstandard installation, put the desired executable first on
`PATH` before running Limitr.

## Troubleshooting

**`could not start codex app-server --stdio`**

Confirm `codex --version` succeeds in the same shell and that the desired Codex
executable is on `PATH`.

**`Unauthenticated`**

Use Codex itself to sign in for the Codex Home named by that Account Profile,
then retry. Limitr cannot and will not authenticate on your behalf.

**`Unsupported: API-key` or `Unsupported: Bedrock`**

That authentication type does not expose ChatGPT rate limits. Register a Codex
Home using ChatGPT authentication if you want it monitored.

**`incompatible Codex app-server`**

Update Codex and retry. The error names the missing method or response field.
Limitr deliberately refuses to guess when required protocol behavior is absent.

**Profile add or configuration errors**

Use an absolute Codex Home path, and choose a label and path not already present
in `limitr profile list`. If TOML was edited manually, correct or move the
platform configuration file and retry.

**The TUI uses ASCII or looks constrained**

Unset `NO_COLOR` and use a Unicode-capable terminal for glyphs. Enlarge the
terminal or use the scrolling controls for more content. ASCII mode retains all
numeric meaning.

## Contributor verification

The same release gate runs on Linux and macOS in GitHub Actions:

```sh
cargo fmt --check
cargo check --all-targets --all-features
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets
```

The integration suite uses fake `codex app-server` executables with fabricated
data. It does not access real credentials, accounts, or network services. The
[MVP verification review](docs/mvp-verification.md) maps the parent
specification's user stories and acceptance boundaries to the executable and
tests.
