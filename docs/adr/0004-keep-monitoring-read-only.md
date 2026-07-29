# Keep monitoring read-only

Limitr will observe identity and rate limits but will not consume reset credits or invoke other account-mutating operations exposed by Codex. Separating observation from account changes prevents a monitoring tool from unexpectedly spending a scarce entitlement, even though users must switch to a Codex-owned surface to perform those actions.

