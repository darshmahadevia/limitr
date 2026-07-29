# Use Codex app-server as the integration boundary

Limitr will obtain identity and current rate limits through Codex app-server's documented `account/read`, `account/rateLimits/read`, and `account/rateLimits/updated` methods. It will not parse the Codex TUI, logs, credential files, or private web endpoints: app-server provides structured data and preserves Codex's ownership of authentication, at the cost of requiring a compatible local Codex installation.

