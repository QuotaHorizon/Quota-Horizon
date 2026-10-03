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

## 独立预测模块

`modules/reset-intelligence/` 可单独运行，使用 Python 3.11+ 与 NumPy 2.x。
它采集公开数据，在自己的 SQLite 中保存原始响应、事件和预测，并输出版本化 JSON。
桌面应用当前仍使用原有雷达；此工具的运行环境和数据目录独立于桌面应用。

在已具备上述依赖的 Python 环境中执行：

```sh
cd modules/reset-intelligence
python -m unittest discover -s tests
python -m reset_intelligence run --db ../../private-data/reset-intelligence/state.sqlite --output ../../private-data/reset-intelligence/forecast.json
python -m reset_intelligence evaluate --db ../../private-data/reset-intelligence/state.sqlite --output ../../private-data/reset-intelligence/evaluation.json
```

`run` 完成一次采集和预测；`watch` 持续运行，每分钟更新预测，每个来源至少间隔 15 分钟采集，
按 `Ctrl+C` 退出。`collect` 只采集，`forecast` 使用已保存的数据。
`replay --as-of <ISO-8601>` 按当时已知的输入与配置回放，要求使用对应的代码版本；
`evaluate` 分别报告历史事件回测和实际发布预测的前瞻评分。
这些命令共用 `--db`；`--output` 原子写入 JSON，`--config` 可指定本地来源与模型参数配置。

输出结构见 [consumer.schema.json](../modules/reset-intelligence/consumer.schema.json)。概率以 `0–1` 表示，
曲线覆盖未来 1–72 小时；目标分为包含发卡的广义额度恢复和直接重置。
消费者应检查 `status`、`valid_until` 和 `training_state`。
`baseline_parameter_interval_80` 表示基准模型参数的不确定性，`component_range` 表示输入之间的分歧。
来源覆盖以外的曲线使用基准补足，并通过 `source_coverage_hours` 和 `alignment` 标识。
输出保留公开来源归属及发言正文；原始采集账本和本地来源配置留在数据处理端。

## 独立评估模块

`modules/reset-observer/` 使用 Python 3.11+ 和 NumPy 2.x 保存预测，独立采集公开完成证据，
并对到期窗口评分。它拥有单独的 SQLite 数据库，可脱离桌面应用和预测包运行。

```sh
cd modules/reset-observer
python -m unittest discover -s tests
python -m reset_observer run --db ../../private-data/reset-observer/state.sqlite --producer-db ../../private-data/reset-intelligence/state.sqlite --output ../../private-data/reset-observer/report.json --html ../../private-data/reset-observer/report.html
```

`--producer-db` 通过只读连接导入预测模块的记录。其他预测器可用 `--ingest predictions.jsonl`，
每行一个[预测对象](../modules/reset-observer/forecast.schema.json)。评估器记录自己的首次接收时间，
并保留原始发布时间。[报告合约](../modules/reset-observer/report.schema.json) 包含当前值、同钟曲线、
事件证据、覆盖率、成绩和成对比较。可选的 HTML 报告作为本地文件打开。

使用相同参数运行 `watch` 可持续评估，`start` 则转入后台。
`status --db <path>` 查看心跳，`stop --db <path>` 请求正常停止。
每分钟检查一次，每个事件渠道至多每 15 分钟采集一次。
预测器可单独持续运行，也可通过 `--config` 中的 `producer_command`（参数数组）
及 `producer_cwd`，按采集周期或预测到期情况续更。评估参数放在 `policy` 中；
变更评估规则需使用独立数据库。后台模式持续到主动停止或电脑重启。

评估采用 UTC 整点起算的 6h、12h、24h、48h 窗口，到期后再等待三小时完成报告。
主成绩对每个期限使用不重叠窗口。来源原值与转换后的模型输入分别记分；
窗口成熟后提供 Brier 分数、对数损失、校准、固定阈值预警，以及相对基准和移除分量
对照组的成对比较。缺失观测保留为空，正负结果均要求连续证据覆盖。
事件更正会追加结算版本。`replay --db <path> --as-of <ISO-8601>` 取回该时刻或此前
保存的报告；内容未变的报告共用一份快照。

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
| `modules/reset-intelligence/` | 独立数据采集、概率估计、回放、评分与 JSON 接口 |
| `modules/reset-observer/` | 独立事件观测、预测冻结、前瞻评估与报告 |
| `fixtures/`、`schemas/` | 测试数据与数据结构 |
| `scripts/` | 构建与源码检查脚本 |
