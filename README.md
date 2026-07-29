# Limitr

Limitr is a local-first CLI/TUI for viewing current Codex usage limits across
multiple Account Profiles.

It is designed to show:

- Account Identity
- Current time
- Usage-limit reset time
- A compact live usage graph

Limitr reads the current state when you use it. It does not use SQLite or
another database, and it does not retain usage history.

## Account Profiles

An Account Profile is a local label and an absolute path to a Codex Home.
Limitr stores that metadata in `limitr/config.toml` under the
platform-appropriate configuration directory. It never copies, reads, changes,
or removes credentials from a Codex Home.

```sh
limitr profile add personal /absolute/path/to/codex-home
limitr profile list
limitr profile remove personal
```

If no configuration file exists, `limitr status` observes a non-persisted
`default` profile from `CODEX_HOME`, or the normal Codex Home when that variable
is unset. With configured profiles, status observes all of them concurrently
and prints them in configured order. Account Profiles that report the same
Account Identity are visibly flagged and remain separate.

## Status exit codes

- `0`: every Account Profile was observed successfully.
- `1`: Limitr could not run the command, such as invalid configuration.
- `2`: status produced a partial result because one or more Account Profiles
  failed. Successful and failed Account Profiles are both included in the
  output.
