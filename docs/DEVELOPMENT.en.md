# Development and builds

[简体中文](DEVELOPMENT.md) · **English**

## Requirements

- Node.js `^20.19.0 || >=22.12.0` and npm.
- Rust stable, configured by `rust-toolchain.toml`.
- macOS: Xcode Command Line Tools; Apple Silicon bundles use the `aarch64-apple-darwin` target.
- Windows: Visual Studio 2022 Build Tools with Desktop development with C++ (MSVC and Windows SDK), the Rust stable MSVC toolchain, and Microsoft Edge WebView2 Runtime.

Frontend dependencies are locked in `apps/desktop/package-lock.json`.
The root `Cargo.lock` belongs to the shared Rust workspace; `apps/desktop/src-tauri/Cargo.lock` belongs to the desktop workspace.

## Install dependencies and run

Run these commands from the repository root:

```sh
npm --prefix apps/desktop ci --no-audit --no-fund
npm run dev:app
```

`dev:app` starts the full desktop application using local accounts and settings.
For browser-only interface development, use `npm run dev` at `http://127.0.0.1:1420`.

## Checks

```sh
npm run check:source
npm run typecheck
npm test
npm run test:scripts
cargo fmt --all -- --check
cargo fmt --manifest-path apps/desktop/src-tauri/Cargo.toml -- --check
cargo test --locked --workspace
cargo test --locked --manifest-path apps/desktop/src-tauri/Cargo.toml
```

`check:source` checks the source file set, lockfiles, document links and Rust path dependencies.
The two Cargo commands cover the shared services and desktop respectively. Add `--offline` to use cached dependencies.
Tests marked `ignored` have individual account, system credential or network requirements documented in their source.

## Standalone forecasting module

`modules/reset-intelligence/` runs independently with Python 3.11+ and NumPy 2.x.
It collects public data, stores raw responses, events and forecasts in its own SQLite database,
and exports versioned JSON. The desktop app currently uses its existing radar;
this tool has a separate runtime and data directory.

Run in a Python environment with those dependencies available:

```sh
cd modules/reset-intelligence
python -m unittest discover -s tests
python -m reset_intelligence run --db ../../private-data/reset-intelligence/state.sqlite --output ../../private-data/reset-intelligence/forecast.json
python -m reset_intelligence evaluate --db ../../private-data/reset-intelligence/state.sqlite --output ../../private-data/reset-intelligence/evaluation.json
```

`run` collects data and produces one forecast. `watch` runs continuously, updates forecasts every minute,
and collects each source at most once per 15 minutes; press `Ctrl+C` to stop.
`collect` only collects; `forecast` uses stored observations.
`replay --as-of <ISO-8601>` replays inputs and configuration known at that time, using the corresponding code revision.
`evaluate` reports retrospective event backtests separately from scores for actual issued forecasts.
All commands take `--db`; `--output` writes JSON atomically, and `--config` selects a local source and model configuration.

The output follows [consumer.schema.json](../modules/reset-intelligence/consumer.schema.json).
Probabilities range from `0–1`, with a curve for the next 1–72 hours. Targets cover broad quota relief,
including banked grants, and automatic resets. Consumers should check `status`, `valid_until` and `training_state`.
`baseline_parameter_interval_80` describes baseline parameter uncertainty; `component_range` describes disagreement among inputs.
The baseline completes unsupported source horizons, identified by `source_coverage_hours` and `alignment`.
The output retains public source attribution and statement text; raw collection ledgers and local source configuration stay with the data processor.

## Standalone evaluation module

`modules/reset-observer/` uses Python 3.11+ and NumPy 2.x to record forecasts,
collect public completion evidence independently, and evaluate mature prediction windows.
It owns a separate SQLite database and runs without the desktop app or the forecasting package.

```sh
cd modules/reset-observer
python -m unittest discover -s tests
python -m reset_observer run --db ../../private-data/reset-observer/state.sqlite --producer-db ../../private-data/reset-intelligence/state.sqlite --output ../../private-data/reset-observer/report.json --html ../../private-data/reset-observer/report.html
```

`--producer-db` imports the forecasting module's records through a read-only connection.
For other producers, `--ingest predictions.jsonl` accepts one [forecast object](../modules/reset-observer/forecast.schema.json)
per line. The evaluator records its own first-receipt timestamp and preserves the original publication time.
The [report contract](../modules/reset-observer/report.schema.json) contains current values, shared-clock curves,
event evidence, coverage, scorecards and paired comparisons. The optional HTML report opens as a local file.

Use `watch` with the same arguments for continuous evaluation, or `start` to detach it into the background.
`status --db <path>` shows its heartbeat; `stop --db <path>` requests a clean stop.
It checks every minute and collects each outcome feed at most once per 15 minutes.
Keep the producer running separately, or use `--config` with `producer_command` (an argument array)
and `producer_cwd` to refresh a producer each collection interval or when its forecasts expire. Policy overrides go in `policy`;
a changed policy requires a separate evaluation database. Background mode lasts until stopped or the computer restarts.

Evaluation uses hourly UTC origins with 6h, 12h, 24h and 48h horizons, then waits a further three hours
for completion reports. Headline scores use non-overlapping windows for each horizon.
Source probabilities and transformed model inputs have separate scorecards. Brier score, log loss,
calibration, fixed-threshold alerts, and paired comparisons against baselines and component-removal
variants become available as windows mature. Missing observations remain missing; both positive and
negative outcomes require continuous evidence coverage. Event corrections append settlement revisions.
`replay --db <path> --as-of <ISO-8601>` retrieves the report saved at or before that time;
unchanged reports share one saved snapshot.

## Build

```sh
npm run build:app -- --target aarch64-apple-darwin
```

`build:app` builds the frontend, Rust executable and application bundle, then checks embedded assets and the local signature.
Use `npm run build` for frontend assets alone.
The macOS bundle is at `apps/desktop/src-tauri/target/aarch64-apple-darwin/release/bundle/macos/QuotaHorizon.app`.
See [installation](INSTALLATION.en.md) for the next steps.

### Windows installer

On a Windows x64 host, run:

```sh
npm --prefix apps/desktop ci --no-audit --no-fund
npm --prefix apps/desktop run build:app:win
```

Tauri automatically merges `tauri.windows.conf.json` to produce an NSIS `.exe` installer instead of a macOS `.app`.
The expected output directory is `apps/desktop/src-tauri/target/x86_64-pc-windows-msvc/release/bundle/nsis/`.
The installer defaults to per-user installation and requires internet access to download WebView2 if the target machine lacks it.
Without Windows code-signing configuration, the package has no publisher signature; this does not establish compatibility with SmartScreen or enterprise policies.

`build:app:win-arm64` additionally requires MSVC ARM64 build tools and the `aarch64-pc-windows-msvc` Rust target.
Passing x64 builds or tests does not verify ARM64. On Windows, `npm run test:scripts` skips the macOS-only POSIX permissions/symlink test.

Windows discovers the official desktop runtime at `%LOCALAPPDATA%/OpenAI/Codex/bin/<build>/codex.exe`, even when it is absent from `PATH`.
The first read requires installation confirmation. The selection stays host-local; restart revalidates its file digest and version, and updates require confirmation again.
History identity uses current-user Windows Credential Manager and the system random generator, without a plaintext key-file fallback.
Mutation locks and restore points use Windows file identity, private DACLs, reparse-point checks and real file/directory flushes.
Restore points currently require a local fixed disk and reject network paths, hard links and unsafe paths; unsupported filesystem flushes fail explicitly.
Conversation and migration regressions use disposable synthetic data, not real account switching, cloud operations or physical power-loss experiments.

The first bundle downloads NSIS tools. If the network requires a proxy, explicitly pass `HTTPS_PROXY` / `HTTP_PROXY` to the current build process.
The bundler may not pick up Windows system proxy settings automatically. Do not disable TLS or hash verification to make downloads work.

The standalone native tray test does not read accounts or use the network:

```sh
cargo run --locked -p capacity-desktop --features native-smoke --example tray_thread_smoke
```

`native-smoke` is only for the standalone test host; the desktop dependency must not enable it, which would link duplicate Windows version resources.
Windows explicitly skips history fixtures requiring Unix file credentials; separate tests cover native credentials, locks, ACL drift, recovery and rollback.
The native credential roundtrip uses a unique random test name, is ignored by default, and deletes its own credential after an explicit run:

```sh
cargo test --locked -p capacity-vault native_unique_credential_roundtrip -- --ignored
```

## Build a DMG

```sh
npm run package:mac
```

This command builds the Apple Silicon app, packages the DMG and verifies an installed copy.
The installer uses the ready-made [create-dmg](https://github.com/sindresorhus/create-dmg) template,
with a drag-and-drop arrow, app icon and Applications folder shortcut. The tool is an optional dependency so its macOS-only dependencies do not block Windows `npm ci`; `package:mac` fails explicitly if the tool is missing.

The DMG installer is at `dist/releases/<version>/QuotaHorizon_<version>_macos-arm64.dmg`.

Use `npm run package:mac -- --skip-build` to repackage an existing app that matches the current source.
Each version has its own output directory; move an existing directory aside before rebuilding the same version.
The script checks the image, installed copy, version, architecture, resources, signature integrity and system library dependencies.

### Packaging configuration

| File | Configuration |
| --- | --- |
| `apps/desktop/src-tauri/tauri.conf.json` | App version, name, macOS 13.0 minimum and bundled resources |
| `apps/desktop/src-tauri/tauri.windows.conf.json` | Windows NSIS installer target and ICO icon |
| `apps/desktop/package.json` | Desktop version, packaging dependencies and Apple Silicon build shortcut |
| `apps/desktop/src-tauri/Cargo.toml` | Desktop Rust package version |
| `scripts/package-macos.mjs` | `aarch64-apple-darwin` target, DMG template, filenames and verification |

The installer version comes from `tauri.conf.json`; keep it aligned with the desktop `package.json`, `Cargo.toml` and their lockfiles.
Tauri produces the `.app`, and `package:mac` wraps it in a DMG.

## Layout

| Path | Contents |
| --- | --- |
| `apps/desktop/` | React/TypeScript interface and Tauri desktop integration |
| `apps/capacity-preview/src-tauri/` | Quota refresh, history, schedule and planning services |
| `apps/capacity-preview/src/status.ts` | Shared frontend IPC types |
| `crates/` | Domain calculations, Codex processes, storage and account operations |
| `modules/reset-intelligence/` | Independent collection, probability estimation, replay, scoring and JSON interface |
| `modules/reset-observer/` | Independent outcome observation, frozen forecasts, prospective evaluation and reports |
| `fixtures/`, `schemas/` | Test data and data structures |
| `scripts/` | Build and source-check scripts |
