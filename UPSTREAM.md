# Upstream source

QuotaHorizon builds on [Codex Switch](https://github.com/piperhex/codex-switch).

- Version: `v1.3.5`
- Commit: `53e4ba14ede178853733e70d1f3a0d66640e9930`
- Source path: `apps/desktop/`
- License: Apache License 2.0

## Changes in QuotaHorizon

- QuotaHorizon application identity, icons and desktop packaging.
- Integrated Rust quota services, local history, work schedules and menu-bar account summaries.
- Public reset collection, source forecasts, post context and event history.
- Account, Provider, session and migration operations with preview and recovery flows.
- Explicit CPA bridge configuration and separate dependency locks for the desktop and shared Rust workspaces.
- Removed Dream Skin assets and the upstream updater; inherited cloud endpoints are disabled by default.

Shared Horizon services are in `crates/` and `apps/capacity-preview/`.
The desktop retains compatibility identifiers used by existing local data.

[LICENSE](LICENSE) and [NOTICE](NOTICE) contain the license and attribution.
Modified files carry change notices where their formats support them.

## Installer template

The macOS disk image uses [create-dmg](https://github.com/sindresorhus/create-dmg)
`8.1.0` by Sindre Sorhus, including its background and icon layout. The packaging
dependency is MIT-licensed and locked in `apps/desktop/package-lock.json`.
