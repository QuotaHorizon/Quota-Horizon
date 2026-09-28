# QuotaHorizon

**简体中文** · [English](README.en.md)

在菜单栏查看 Codex 额度，在一个窗口里管理账号、日程、历史和会话。
QuotaHorizon 还会自动汇集重置预测和相关公开发言，方便随时了解额度与重置动态。

## 下载与安装

[**下载 macOS 版**](https://github.com/QuotaHorizon/Quota-Horizon/releases) · [安装帮助](docs/INSTALLATION.md)

适用于 macOS 13 及以上、Apple Silicon（M 系列）Mac。

1. 从 Releases 下载 macOS Apple Silicon 的 `.dmg` 安装包。
2. 打开 DMG，将 `QuotaHorizon.app` 拖入 Applications。
3. 从“应用程序”打开 QuotaHorizon，按提示连接本机已登录的 Codex。

## 功能

- **菜单栏监控**：后台刷新额度，显示剩余比例和重置时间；关窗后继续运行。
- **多账号总览**：并列查看各账号额度、套餐和刷新状态，按需切换登录。
- **额度历史**：趋势图与活动热图，使用虚线区分额度回升。
- **工作日程**：设置工作和停用时段，将剩余额度分配到每天。
- **会话管理**：搜索、阅读、继续会话，以及归档、恢复和修复。
- **重置雷达**：查看来源网站的 24/48 小时预测、Tibo 发言、回复上下文和事件日历。
- **Provider 与 CPA**：管理 API 连接，并接入自行配置的 CPA 额度池。

## 界面

以下为实际产品组件的合成数据预览。

**菜单栏弹窗**：额度、重置倒计时与账号入口。

<img src="docs/images/menu-popover-preview.png" alt="菜单栏弹窗" width="460">

**多账号总览**：各账号的额度、套餐和重置时间。

![多账号总览](docs/images/accounts-preview.png)

**重置雷达**：来源概率、相关发言与回复上下文。

![重置雷达](docs/images/reset-radar-preview.png)

**额度历史**：用量趋势和活动热图。

![额度历史](docs/images/history-light.png)

[查看深色界面](docs/images/history-dark.png)

## 数据与配置

账号、历史和会话保存在本机。重置雷达自动汇集公开信息，标注来源与更新时间。

[配置说明](docs/CONFIGURATION.md) · [重置雷达](docs/CONFIGURATION.md#重置雷达) · [更新与历史访问](docs/INSTALLATION.md#更新)

## 从源码构建

本地运行、构建安装包和测试命令见[开发指南](docs/DEVELOPMENT.md)。

## 开源

QuotaHorizon 基于 Codex Switch `v1.3.5` 构建，是独立于 OpenAI 的开源项目。
采用 [Apache-2.0](LICENSE) 许可证，来源与修改说明见 [NOTICE](NOTICE) 和 [UPSTREAM.md](UPSTREAM.md)。
