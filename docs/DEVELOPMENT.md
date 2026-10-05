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
运行环境和数据目录独立于桌面应用；桌面通过下方的 JSON 连接读取结果。

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
`audit --db <path> --output audit.json` 只读审查已发布预测的来源采用情况、采集时间、
原始支持期限及社区权重。`run` / `watch` 增加 `--audit-output audit.json` 可持续输出这份审查。
默认拟合结果保留为待审候选，已发布组合继续使用固定先验；配置中的
`model.apply_learned_weights: true` 可显式启用满足训练条件后的拟合权重。

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
每分钟检查一次，正常事件渠道每 15 分钟采集。临时请求或解析失败按 1、2、4、8、15 分钟
退避重试；限流、访问受限和成功读取但内容过旧的渠道保持 15 分钟间隔。
预测器可单独持续运行，也可通过 `--config` 中的 `producer_command`（参数数组）
及 `producer_cwd`，按采集周期或预测到期情况续更。评估参数放在 `policy` 中；
变更评估规则需使用独立数据库。后台模式持续到主动停止或电脑重启。

评估采用 UTC 整点起算的 6h、12h、24h、48h 窗口，到期后再等待三小时完成报告。
主成绩对每个期限使用不重叠窗口。来源原值与转换后的模型输入分别记分；
窗口成熟后提供 Brier 分数、对数损失、校准、固定阈值预警，以及相对基准和移除分量
对照组的成对比较。缺失观测保留为空，正负结果均要求连续证据覆盖。
事件更正会追加结算版本。`replay --db <path> --as-of <ISO-8601>` 取回该时刻或此前
保存的报告；内容未变的报告共用一份快照。
报告的 `diagnostics` 区分断采、请求失败、源站内容过旧和同内容重复读取，按实际事件窗口
归并多个来源的检验记录。`diagnose --db <path> --origin <ISO-8601> --hours 6` 可只读检查
指定窗口的覆盖缺口；`--hours` 也支持 12、24、48。解析规则修正追加当前知识版本，保留原始
观测时间和旧报告。`dependence` 提供同一时钟的水平及变化相关性。
预先固定的社区 25% / 50% / 75% 候选与移除社区、历史基线、等权组合并行记录，
以相同窗口的 Brier 和对数损失比较，评估器不自动替换预测器权重。

## 接入桌面雷达

macOS 安装包包含独立模块及运行环境。首次打开即在应用数据目录创建各自的数据库与 JSON，
后台启动和恢复采集；升级时旧进程完成当前轮次后退出，再启动新版本，数据保留原位。
已有自定义连接和运行配置继续生效。以下命令用于开发或连接另一份独立输出。

预测与评估进程持续运行后，在仓库根目录连接它们的 JSON 输出：

```sh
node scripts/connect-reset-modules.mjs --forecast /absolute/path/forecast.json --evaluation /absolute/path/report.json
```

脚本验证合约版本，并将只读路径写入应用数据目录中的 `reset-radar-connection.json`。
可用 `--data-dir /absolute/app/data` 指定另一份应用数据目录。默认目录为 macOS 的
`~/Library/Application Support/io.github.halfmelon.quotahorizon`、Windows 的
`%APPDATA%/io.github.halfmelon.quotahorizon` 或 Linux 的
`$XDG_DATA_HOME/io.github.halfmelon.quotahorizon`（默认 `~/.local/share`）。
也可直接将输出放在该目录的 `reset-intelligence/consumer.json` 和 `reset-observer/report.json`，
应用会自动读取。

Horizon 启动时读取结果，此后在后台每分钟检查变化；主窗口和菜单栏共享同一份估计。
它校验结构、概率和时间，筛选可展示字段，读取失败时保留带状态的上次结果。
自定义外部模块由各自的 `start` / `watch` 命令维护进程。也可让 Horizon 在运行时检查并恢复独立评估器：

```sh
node scripts/connect-reset-modules.mjs --forecast /absolute/path/forecast.json --evaluation /absolute/path/report.json \
  --python /absolute/path/python --observer-dir /absolute/path/modules/reset-observer \
  --observer-db /absolute/path/observer.sqlite --producer-db /absolute/path/producer.sqlite \
  --config /absolute/path/observer-config.json --html /absolute/path/report.html
```

脚本验证现有 Python 环境和已初始化的数据文件，再写入 `reset-radar-runtime.json`。
`--producer-db`、`--config`、`--html` 可选；需要自动更新预测时，在评估器配置中提供上述
`producer_command` 和 `producer_cwd`。Horizon 约每分钟通过独立模块的 `start` 检查单实例，
进程退出后自动重启，启动失败时逐步延长重试间隔至最多 15 分钟。
更改已托管的输出路径、输入数据库或配置后，下一次检查会请求旧进程正常退出，再按新参数启动；
原数据库与报告保留。
电脑重启后，打开 Horizon 即可恢复这些进程；退出 Horizon 后，已经启动的独立进程继续运行。
概览页显示进程状态，输出新鲜度仍按实际更新时间显示。

需要自行管理或停止模块时，先重新执行 JSON 连接命令并加 `--external`，再运行模块的 `stop`。
这会关闭自动恢复，保留进程和数据。桌面只读取 JSON，由模块进程管理数据库与模型。
移除自定义连接与运行配置后，macOS 应用恢复自带模块。原有事件历史与阅读状态保留。

## 构建

```sh
export QUOTAHORIZON_PYTHON=/absolute/path/to/conda/environment/bin/python
npm run build:app -- --target aarch64-apple-darwin
```

`build:app` 构建前端、Rust 可执行文件和应用包，并校验内嵌资源与本地签名。
macOS 构建使用已安装的 Conda Python 3.11–3.14 和 NumPy 2.x，所有二进制需兼容 macOS 13。
脚本将标准库、NumPy、两个模块、所需动态库和许可文件复制到构建产物，校验独立运行与系统版本。
所用 Conda 包缓存需保留 `info/licenses`。脚本不安装依赖；私有来源配置与数据库留在应用包外。
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
