# Changelog

## v1.2.117 — 2026-04-23

### New Features

- **Gateway routing strategy** — Each account can choose its routing mode (Proxy / Vercel / Direct); in Proxy mode, a country can be specified per account and the residential proxy binds to that country (US / JP / KR / PH); automatic fallback when the preferred route is unavailable

### Improvements

- **Claude Code CLI 2.1.117 support** — MAX_SUPPORTED_VERSION upgraded from 2.1.114
- **Prompt cache hit rate** — Session ID handling fixes reduce cache misses in large sessions
- **Audit logging** — Gateway now records request/response headers for diagnostics
- **Startup & config robustness** — Start gateway with 0 accounts; LAN subnet whitelisting supports CIDR; IPv6 host bracketing; secure_fs credential files at 0600 permissions

### Bug Fixes

- Updater ACL fix (auto-update was broken in v1.1.x)
- Stream intent preservation (no forced SSE)
- Hop-by-hop header stripping
- Multiple i18n locale entries

## v1.1.0 — 2026-04-18

### Security

- **Atomic secure writes** — All credential files (accounts, config, auth) now use atomic temp+rename with 0600 permissions at creation time, eliminating the umask window where credentials were briefly world-readable
- **LAN subnet whitelisting** — RFC1918 IPs auto-expand to /24 CIDR; CGNAT/Tailscale/public IPs restricted to single-IP only

### Bug Fixes

- **Empty `anthropic-beta` header** — Fixed leading comma when header value was empty
- **Session key guard** — Web login dialog now validates `sessionKey` before processing

### Improvements

- **Version policy gate** — Inbound requests validated for CC CLI version consistency; future versions auto-clamped to `MAX_SUPPORTED_VERSION`
- **Stream intent preservation** — Gateway respects client's original `stream` field instead of forcing SSE
- **In-place metadata rewrite** — Body parsed once across retry loop, eliminating redundant serialize/deserialize per attempt
- **Opus 4.7 support** — Model detection and pricing ($5/$25 per MTok, same as Opus 4.6)

## v1.0.0 — 2026-04-16

First public release.

### Features

- **Local API Gateway** — Transparent HTTP proxy on `localhost:9000` for Claude Code CLI
- **Multi-Account Pool** — Add and manage multiple Claude accounts with automatic rotation
- **Quota Monitoring** — Real-time session (5h) and weekly (7d) usage tracking
- **Automatic Failover** — Seamless retry on the next available account
- **Residential Proxy** — IPRoyal proxy pool with per-session sticky IPs
- **Traffic Logs** — Full request/response logging with SQLite storage
- **GitHub Login** — Device flow authentication
- **License Subscription** — Free (1) / Pro (3) / Max (10) / Ultra (unlimited) concurrent accounts
- **Web Login & OAuth** — Browser automation for account onboarding (via bundled webapp in .dmg)
- **12 Languages** — EN, 简体中文, 繁體中文, 日本語, 한국어, العربية, ES, PT, RU, TR, VI, MY

### Architecture

- Tauri 2 (Rust backend + React frontend)
- BoringSSL TLS via `boring` crate
- Public repository uses stub `http/` crate for code review; official `.dmg` includes full implementation
