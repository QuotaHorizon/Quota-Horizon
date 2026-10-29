# 开发与构建

**简体中文** · [English](DEVELOPMENT.en.md)

## 环境

- Node.js `^20.19.0 || >=22.12.0` 与 npm。
- Rust stable，工具链配置见 `rust-toolchain.toml`。
- macOS：Xcode Command Line Tools；Apple Silicon 打包使用 `aarch64-apple-darwin` target。
- Windows：Visual Studio 2022 Build Tools 的“使用 C++ 的桌面开发”（MSVC 与 Windows SDK）、Rust stable MSVC 工具链，以及 Microsoft Edge WebView2 Runtime。

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

### GitHub Actions

[Desktop CI](../.github/workflows/desktop.yml) 在向 `dev`、`main` 推送或发起拉取请求时
测试并打包 macOS Apple Silicon。手动运行可选择 `macos-arm64` 或 `windows-x64`；
每次只构建选中的平台，分别生成 DMG 或 NSIS `.exe`。
两端均测试前端、Rust 以及独立预测和评估模块。
[macos-runtime.conda.lock](../.github/ci/macos-runtime.conda.lock) 与
[windows-runtime.lock.json](../.github/ci/windows-runtime.lock.json) 使用 URL 和 SHA-256 锁定运行时下载。
打包时检查运行环境隔离和原生架构，其中 Apple Silicon 还检查 macOS 13 兼容性。
Windows 任务静默安装 NSIS 包，并测试安装后的运行环境与后台进程生命周期。

可从该次运行的产物下载安装器；名称包含平台与源码提交，保留 14 天。
DMG 任务检查内置运行环境、签名、磁盘映像和安装副本；两端都检查原生可执行文件
是否包含当前前端资源。公开安装器仍从
[Releases](https://github.com/QuotaHorizon/Quota-Horizon/releases) 下载。
发布前，使用同一源码和桌面版本分别完成两端任务。

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

`run` 完成一次采集和预测；`watch` 每分钟检查主要来源，其他来源使用各自采集间隔，
按 `Ctrl+C` 退出。`collect` 只采集，`forecast` 使用已保存的数据。
`run --refresh` 主动采集后重算，保留 5 秒重复请求保护和失败退避。
`replay --as-of <ISO-8601>` 按当时已知的输入与配置回放，要求使用对应的代码版本；
`evaluate` 分别报告历史事件回测和实际发布预测的前瞻评分。
这些命令共用 `--db`；`--output` 原子写入 JSON，`--config` 可指定本地来源与模型参数配置。
`audit --db <path> --output audit.json` 只读审查已发布预测的来源采用情况、采集时间、
原始支持期限及社区权重。`run` / `watch` 增加 `--audit-output audit.json` 可持续输出这份审查。
当前组合使用配置先验，`model.apply_learned_weights` 保持 `false`；
已发布预测和对照模型的前瞻成绩由评估器记录。

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
每轮完成后等待 60 秒，再检查和采集正常事件渠道。临时请求或解析失败按 1、2、4、8、15 分钟
退避重试；限流和访问受限保留至少 15 分钟间隔。
`refresh` 使用与 `start` 相同的参数，唤醒现有单实例进程，更新采集、预测与评估。
预测器可单独持续运行，也可通过 `--config` 中的 `producer_command`（参数数组）
及 `producer_cwd`，按采集周期或预测到期情况续更。评估参数放在 `policy` 中；
变更评估规则需使用独立数据库。后台模式持续到主动停止或电脑重启。

评估采用 UTC 整点起算的 6h、12h、24h、48h 窗口，到期后再等待三小时完成报告。
主成绩对每个期限使用不重叠窗口。来源原值与转换后的模型输入分别记分；
窗口成熟后提供 Brier 分数、对数损失、校准、固定阈值预警，以及相对基准和移除分量
对照组的成对比较。缺失观测保留为空，正负结果均要求连续证据覆盖。
事件更正会追加结算版本。`replay --db <path> --as-of <ISO-8601>` 取回该时刻或此前
保存的报告。实时报告每轮更新；归档保留最近七天各小时的最后一份报告，更早按日保留，
并保留每个实现与评估规则版本的首尾报告。回放使用报告自身的真实生成时间。
原始接口整包缓存保留48小时，随后保留30天内的每日快照及各来源最新可用响应，
自动清理过期整包。结构化输入、采集时钟、投票、原始预测、冻结试验与事件/结算修订长期保留。
超过48小时且已失效、未被冻结试验引用的影子与对齐中间试算自动清理；已用于正式评分或
社区检验的完整预测持续保留。清理后的试算仍保留首次接收时间和去重标识，累计采集计数连续。
报告的 `diagnostics` 区分断采、请求失败、源站内容过旧和同内容重复读取，按实际事件窗口
归并多个来源的检验记录。`diagnose --db <path> --origin <ISO-8601> --hours 6` 可只读检查
指定窗口的覆盖缺口；`--hours` 也支持 12、24、48。解析规则修正追加当前知识版本，保留原始
观测时间和报告归档。`dependence` 提供同一时钟的水平及变化相关性。
预先固定的社区 25% / 50% / 75% 候选与移除社区、历史基线、等权组合并行记录，
以相同窗口的 Brier 和对数损失比较，评估器不自动替换预测器权重。

## 接入桌面雷达

macOS 与 Windows x64 安装包包含独立模块及运行环境。首次打开即在应用数据目录创建各自的数据库与 JSON，
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
移除自定义连接与运行配置后，应用恢复自带模块。原有事件历史与阅读状态保留。

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

### Windows 安装包

在 Windows x64 主机上运行：

```sh
npm --prefix apps/desktop ci --no-audit --no-fund
node scripts/prepare-windows-runtime.mjs
npm --prefix apps/desktop run build:app:win
```

Tauri 自动合并 `tauri.windows.conf.json`，生成 NSIS `.exe` 安装包，而不是 macOS `.app`。
预期输出目录为 `apps/desktop/src-tauri/target/x86_64-pc-windows-msvc/release/bundle/nsis/`。
安装包默认按用户安装；目标机器缺少 WebView2 时，安装程序需要联网下载运行时。
未配置 Windows 代码签名的包没有发布者签名，不能据此认为 SmartScreen 或企业策略兼容性已经验证。

准备脚本将固定版本的官方嵌入式 CPython、NumPy wheel 与时区数据下载到 `dist/`，
校验哈希并保留再分发许可文件，系统 Python 和 `PATH` 保持不变。
构建将运行时与两个模块一同打包，校验 x64 二进制、TLS 证书、时区数据和 NumPy，运行无需开发环境。
Windows ARM64 安装包暂未提供。`npm run test:scripts` 在 Windows 上跳过 macOS 的 POSIX 权限/符号链接测试。

Windows 会发现官方桌面应用导出的 `%LOCALAPPDATA%/OpenAI/Codex/bin/<构建>/codex.exe`，即使它不在 `PATH` 中。
首次使用需要确认读取来源；确认记录仅保存在本机宿主，重启时重新验证文件摘要与版本，读取组件更新后须再次确认。
历史账号绑定使用当前用户的 Windows Credential Manager 和系统安全随机数，不回退为明文密钥文件。
变更互斥与还原点使用 Windows 文件身份、私有 DACL、重解析点检查和真实文件/目录同步。
恢复点目前限本地固定磁盘，拒绝网络路径、硬链接及不安全路径；不支持目录同步的文件系统会明确报错。
会话及迁移回归使用临时合成数据，不替代真实账号切换、云端操作或物理断电实验。

首次打包需要下载 NSIS 工具；若本机网络要求代理，需向当前构建进程显式提供 `HTTPS_PROXY` / `HTTP_PROXY`。
Windows 系统代理不一定被打包工具自动采用。不要为了下载关闭 TLS 或哈希校验。

独立原生托盘测试不读取账号或联网：

```sh
cargo run --locked -p capacity-desktop --features native-smoke --example tray_thread_smoke
```

`native-smoke` 只用于独立测试宿主；正式桌面依赖不启用它，以免链接重复的 Windows 版本资源。
Windows 测试明确跳过依赖 Unix 文件凭据的历史夹具测试；另有原生凭据、互斥、权限漂移、恢复与回滚测试。
凭据库真实读写测试使用随机独立名称，默认 `ignored`，显式运行后删除自己的测试凭据：

```sh
cargo test --locked -p capacity-vault native_unique_credential_roundtrip -- --ignored
```

## 构建 DMG

```sh
npm run package:mac
```

该命令完成 Apple Silicon 应用构建、DMG 封装和安装副本校验。
安装窗口使用 [create-dmg](https://github.com/sindresorhus/create-dmg) 的现成模板，
带拖拽箭头、应用图标和 Applications 文件夹入口。打包工具作为可选依赖安装，避免 macOS 专用依赖阻塞 Windows 的 `npm ci`；缺少工具时 `package:mac` 会明确报错。

DMG 安装包位于 `dist/releases/<version>/QuotaHorizon_<version>_macos-arm64.dmg`。

重新封装对应当前源码的既有应用包，使用 `npm run package:mac -- --skip-build`。
每个版本使用独立输出目录；重新生成同版本安装包前，先将已有目录移走保存。
脚本检查映像、安装副本、版本、架构、资源、签名完整性和系统动态库依赖。

### 打包配置

| 文件 | 配置 |
| --- | --- |
| `apps/desktop/src-tauri/tauri.conf.json` | 应用版本、名称、macOS 13.0 最低版本及内嵌资源 |
| `apps/desktop/src-tauri/tauri.windows.conf.json` | Windows NSIS 安装包目标与 ICO 图标 |
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
