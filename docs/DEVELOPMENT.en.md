# Development and builds

[简体中文](DEVELOPMENT.md) · **English**

## Requirements

- Node.js `^20.19.0 || >=22.12.0` and npm.
- Rust stable, configured by `rust-toolchain.toml`.
- macOS: Xcode Command Line Tools; Apple Silicon bundles use the `aarch64-apple-darwin` target.

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

## Build

```sh
npm run build:app -- --target aarch64-apple-darwin
```

`build:app` builds the frontend, Rust executable and application bundle, then checks embedded assets and the local signature.
Use `npm run build` for frontend assets alone.
The macOS bundle is at `apps/desktop/src-tauri/target/aarch64-apple-darwin/release/bundle/macos/QuotaHorizon.app`.
See [installation](INSTALLATION.en.md) for the next steps.

## Build a DMG

```sh
npm run package:mac
```

This command builds the Apple Silicon app, packages the DMG and verifies an installed copy.
The installer uses the ready-made [create-dmg](https://github.com/sindresorhus/create-dmg) template,
with a drag-and-drop arrow, app icon and Applications folder shortcut. The tool is installed with the project's development dependencies.

The DMG installer is at `dist/releases/<version>/QuotaHorizon_<version>_macos-arm64.dmg`.

Use `npm run package:mac -- --skip-build` to repackage an existing app that matches the current source.
Each version has its own output directory; move an existing directory aside before rebuilding the same version.
The script checks the image, installed copy, version, architecture, resources, signature integrity and system library dependencies.

### Packaging configuration

| File | Configuration |
| --- | --- |
| `apps/desktop/src-tauri/tauri.conf.json` | App version, name, macOS 13.0 minimum and bundled resources |
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
| `fixtures/`, `schemas/` | Test data and data structures |
| `scripts/` | Build and source-check scripts |
