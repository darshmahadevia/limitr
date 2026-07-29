# Limit Monitoring

Limitr gives one person a present-tense view of Codex capacity across the account profiles available on their machine. Its language distinguishes a local profile from the remote identity and quota data observed through that profile.

## Language

**Account Profile**:
A locally named connection to one Codex authentication context. Two profiles may resolve to the same account identity and remain distinct profiles.
_Avoid_: Account, login, user

**Codex Home**:
The credential and configuration boundary used by Codex for one account profile.
_Avoid_: Account directory, Limitr account

**Account Identity**:
The remote identity reported by Codex for an authenticated profile, such as an email address and plan.
_Avoid_: Profile, username

**Limit Snapshot**:
The latest observed set of rate-limit values for an account profile, together with when it was observed.
_Avoid_: Usage record, history entry

**Limit Bucket**:
A separately metered category of Codex capacity reported for an account identity.
_Avoid_: Plan, allowance

**Quota Window**:
A bounded interval within a limit bucket that reports a used percentage and reset instant.
_Avoid_: Timer, period

**Reset Instant**:
The absolute moment at which a quota window is expected to reset.
_Avoid_: Reset duration, refresh time

**Live Trace**:
A bounded, in-memory sequence of observations collected during the current Limitr process for drawing a compact graph. It ceases to exist when that process exits.
_Avoid_: Usage history, analytics

**Stale Snapshot**:
The last successful limit snapshot retained in memory after its profile can no longer be refreshed, clearly marked with its observation age.
_Avoid_: Current usage, cached history

