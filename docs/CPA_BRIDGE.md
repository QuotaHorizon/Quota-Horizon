# CPA 额度池 bridge

**简体中文** · [English](CPA_BRIDGE.en.md)

Provider 页面可通过 SSH 查询 CPA 额度池，显示账号额度、路由状态和最近刷新结果。

## 配置

1. 准备 CPA 服务端的只读 JSON 查询命令，并在本机配置可用的 SSH 连接。
2. 在 Provider 页面保存 SSH 目标，例如 `operator@cpa-host`。
3. 为 Horizon 进程设置 `QUOTA_HORIZON_CPA_QUOTA_COMMAND`，值为服务端查询命令。
4. 启用 bridge 并刷新额度池。

开发启动示例：

```sh
QUOTA_HORIZON_CPA_QUOTA_COMMAND='/opt/cpa/bin/quota --json' npm run dev:app
```

将示例路径替换为服务端的实际命令。从 Finder 启动时，需另外确保应用进程收到该环境变量。
已有的 `CODEX_QUOTA_VIEWER_CPA_QUOTA_COMMAND` 仍兼容；新变量优先。

查询命令会在指定主机执行，请使用可信服务提供的只读命令。SSH 使用 `BatchMode=yes`，
连接需预先完成认证。默认刷新间隔为 300 秒、超时为 75 秒，界面可设置范围分别为
60–3600 秒和 2–120 秒。

## 响应格式

服务端输出 JSON，大小上限为 2 MiB。主要字段为 `current`、`latest` 和可选的 `accounts`。
账号条目包括 `id`、`display_name`、可选的 `latest` 额度观测，以及路由、保护和刷新状态。
额度从 `codex_headers` 读取；响应只需包含额度与状态，凭据留在服务端。

解析器和完整测试样例位于 `apps/desktop/src-tauri/src/cpa_pool.rs`。
刷新失败时保留上次快照和采集时间；停用 bridge 后保留已有缓存和 Provider 配置。
