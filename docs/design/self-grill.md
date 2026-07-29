# Limitr self-grill

This is the converged result of repeatedly questioning the product boundary, data source, account model, failure behavior, and user experience. Answers are recommendations made on the user's behalf; implementation has not begun.

## Product boundary

**Who is the first user?**  
An individual developer who uses multiple ChatGPT-backed Codex accounts on one machine and wants to know which account has capacity before starting work.

**What job must Limitr do?**  
Answer “what capacity is available right now, for which identity, and when does it reset?” without opening a Codex session for every account.

**Is this an analytics product?**  
No. It is a present-tense monitor. Trends across launches, reports, exports of past usage, and usage forecasting are deliberately outside the product.

**What does local-first mean here?**  
Limitr runs locally, obtains account data through local Codex processes, and needs no Limitr service or hosted backend. Codex may still contact OpenAI to refresh authenticated account data.

**What interfaces define the MVP?**  
`limitr` opens the interactive TUI. `limitr status` produces a one-shot terminal snapshot, with a machine-readable form reserved for deliberate schema design rather than accidental stabilization.

## Source of truth

**Should Limitr scrape `/status`, parse log files, read token files, or call a private web endpoint?**  
None of those. Codex app-server documents `account/read`, `account/rateLimits/read`, and `account/rateLimits/updated`; these are the source of truth.

**Should Limitr calculate percentage used from local token counts?**  
No. Server-reported `usedPercent` is authoritative because quotas need not map directly to locally visible token counts.

**What if Codex changes its response shape?**  
Treat unknown fields as forward-compatible, validate required fields defensively, and surface a per-profile protocol error without taking down other profiles.

**Should Limitr use `account/usage/read` to make the graph richer?**  
No. That endpoint exposes historical token activity and conflicts with the product's no-history boundary. The graph uses only live rate-limit observations from the current process.

## Multiple accounts

**What exactly is an “account” in configuration?**  
An Account Profile: a user-chosen label plus a Codex Home. The authenticated email and plan are observed Account Identity, not configuration identity.

**How can multiple accounts coexist without Limitr handling tokens?**  
Each profile points to a distinct Codex Home. Limitr launches or connects to a Codex app-server in that boundary and never opens the credential store itself.

**Can two profiles resolve to the same remote account?**  
Yes. Profiles remain distinct, but the TUI should warn that their reported account identities match so the user does not mistake them for separate capacity.

**What if a profile uses API-key or Bedrock authentication?**  
Show the identity/auth mode and mark ChatGPT rate limits as unsupported or unavailable. Never fabricate an equivalent percentage.

**Does Limitr own sign-in?**  
Not in the monitoring core. Initial setup may invoke Codex's supported login flow, but Codex owns authentication, credential persistence, and refresh.

## “Live” behavior

**How fresh is live?**  
Subscribe to `account/rateLimits/updated` and periodically reconcile with `account/rateLimits/read`. A conservative default refresh interval should avoid needless requests and remain configurable.

**What does the compact graph represent?**  
Used percentage over observations made during this process, independently for each quota window. It is a sparkline, not an analytics chart.

**Is an in-memory sparkline “usage history”?**  
It is a Live Trace, not retained history: bounded, process-local, never serialized, and discarded on exit. This is the minimum state required to draw change over time.

**What happens at reset?**  
A drop in used percentage is rendered as observed. Limitr does not smooth it away or infer a reset before Codex reports one.

**What time is displayed?**  
One continuously updating local wall clock in the TUI header. Every reset is shown as both a local absolute time and a relative countdown, derived from the server's reset instant.

**How are clock changes handled?**  
Recompute countdowns from the absolute reset instant and current system time. Do not decrement a stored countdown as if it were truth.

## Display model

**What is the smallest useful account view?**  
Profile label, reported identity and plan, connection state, observation age, then every reported limit bucket and its quota windows with used percentage, reset time, and Live Trace.

**Should only the legacy primary bucket be shown?**  
No. Prefer the multi-bucket response when present and fall back to the backward-compatible single bucket. Silently hiding a metered bucket could mislead the user.

**How should partial failure look?**  
One profile may be loading, ready, stale, unauthenticated, unsupported, or errored while all others continue updating.

**Should the last good value disappear on a transient error?**  
No. Keep one Stale Snapshot in memory, label it stale prominently, and show its age. On process restart there is no snapshot to recover.

**Can color carry meaning?**  
No. Text labels, symbols, and percentages must preserve meaning in monochrome; color is enhancement only.

## Safety and privacy

**May Limitr consume earned reset credits?**  
No. Monitoring is read-only. It must not call reset-credit consumption or other account-mutating endpoints.

**What may be persisted?**  
Only configuration needed to locate and label profiles plus harmless display preferences. No Limit Snapshot, Live Trace, token activity, access token, refresh token, or copied credential material.

**Should Limitr read `auth.json` to discover emails faster?**  
No. That file contains secrets. Identity comes from `account/read`.

**What should logs contain?**  
Operational state and sanitized errors. Never protocol payloads containing credentials; account email should be redacted unless the user explicitly enables diagnostic identity output.

## Operations and failure

**What if one Codex app-server hangs?**  
Timeout and back off that profile independently. Other profiles and the TUI clock remain responsive.

**What if Codex is missing or too old for the required methods?**  
Report a precise compatibility error and the detected Codex version. Do not fall back to credential parsing or screen scraping.

**What happens on terminal resize or a large account list?**  
Cards compact responsively; the list scrolls; important numeric values remain available without relying on the graph.

**How does the process exit?**  
Stop child app-servers it started, discard all snapshots and traces, leave Codex Homes untouched, and write no usage state.

## Convergence test

The design is coherent if all of these scenarios behave predictably:

1. Two healthy profiles update independently and show distinct identities.
2. Two profiles reveal the same identity and are visibly flagged as duplicates.
3. One profile loses network access; its last snapshot becomes stale while the other remains live.
4. A quota window resets; the reported drop appears in its ephemeral trace.
5. Limitr restarts; configuration remains, but graphs and previous values are empty.
6. A profile is API-key-only; identity/auth mode remains visible and ChatGPT quota is explicitly unsupported.
7. Codex adds another limit bucket; it appears without requiring a hard-coded bucket name.
8. The terminal has no color and is narrow; identity, percent used, and reset information remain understandable.

No unresolved product decision blocks an MVP plan. Technology selection, exact refresh defaults, configuration syntax, and a machine-readable output schema should be decided immediately before implementation because they are comparatively reversible and benefit from repository/toolchain constraints.

