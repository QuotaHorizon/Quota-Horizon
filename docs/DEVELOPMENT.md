# 开发与构建

**简体中文** · [English](DEVELOPMENT.en.md)

## 环境

- Node.js `^20.19.0 || >=22.12.0` 与 npm。
- Rust stable，工具链配置见 `rust-toolchain.toml`。
- macOS：Xcode Command Line Tools；Apple Silicon 打包使用 `aarch64-apple-darwin` target。

前端依赖锁定在 `apps/desktop/package-lock.json`。
根 `Cargo.lock` 对应共享 Rust workspace，`apps/desktop/src-tauri/Cargo.lock` 对应桌面 workspace。

## 安装依赖与启动

以下命令均在仓库根目录执行：

```sh
npm --prefix apps/desktop ci --no-audit --no-fund
npm run dev:app
```

`dev:app` 启动完整桌面应用，使用本机账号和设置。仅调试浏览器界面时使用 `npm run dev`，
默认地址为 `http://127.0.0.1:1420`。

## 检查

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

`check:source` 检查文件范围、锁文件、文档链接和 Rust 路径依赖。
两个 Cargo 命令分别运行共享服务和桌面测试。Cargo 可加 `--offline` 使用本地缓存。
标记为 `ignored` 的测试有各自的账号、系统凭据或网络条件，运行说明位于测试源码中。

## 构建

```sh
npm run build:app -- --target aarch64-apple-darwin
```

`build:app` 构建前端、Rust 可执行文件和应用包，并校验内嵌资源与本地签名。
单独构建前端可使用 `npm run build`。
macOS 产物位于 `apps/desktop/src-tauri/target/aarch64-apple-darwin/release/bundle/macos/QuotaHorizon.app`。
安装步骤见[安装指南](INSTALLATION.md)。

## 构建 DMG

```sh
npm run package:mac
```

该命令完成 Apple Silicon 应用构建、DMG 封装和安装副本校验。
安装窗口使用 [create-dmg](https://github.com/sindresorhus/create-dmg) 的现成模板，
带拖拽箭头、应用图标和 Applications 文件夹入口。打包工具随项目开发依赖安装。

DMG 安装包位于 `dist/releases/<version>/QuotaHorizon_<version>_macos-arm64.dmg`。

重新封装对应当前源码的既有应用包，使用 `npm run package:mac -- --skip-build`。
每个版本使用独立输出目录；重新生成同版本安装包前，先将已有目录移走保存。
脚本检查映像、安装副本、版本、架构、资源、签名完整性和系统动态库依赖。

### 打包配置

| 文件 | 配置 |
| --- | --- |
| `apps/desktop/src-tauri/tauri.conf.json` | 应用版本、名称、macOS 13.0 最低版本及内嵌资源 |
| `apps/desktop/package.json` | 桌面版本、打包依赖和 Apple Silicon 构建快捷命令 |
| `apps/desktop/src-tauri/Cargo.toml` | 桌面 Rust 包版本 |
| `scripts/package-macos.mjs` | `aarch64-apple-darwin` 目标、DMG 模板、文件命名与校验 |

安装包版本取自 `tauri.conf.json`，与桌面 `package.json`、`Cargo.toml` 及对应锁文件同步。
Tauri 生成 `.app`，`package:mac` 将它封装为 DMG。

## 目录

| 路径 | 内容 |
| --- | --- |
| `apps/desktop/` | React/TypeScript 界面与 Tauri 桌面集成 |
| `apps/capacity-preview/src-tauri/` | 额度刷新、历史、日程与规划服务 |
| `apps/capacity-preview/src/status.ts` | 前端共享的 IPC 类型 |
| `crates/` | 领域计算、Codex 进程、存储和账号操作 |
| `fixtures/`、`schemas/` | 测试数据与数据结构 |
| `scripts/` | 构建与源码检查脚本 |
