# 安装与首次运行

**简体中文** · [English](INSTALLATION.en.md)

适用于 macOS 13 及以上、Apple Silicon（M 系列）Mac。

## 安装

安装包见 [GitHub Releases](https://github.com/QuotaHorizon/Quota-Horizon/releases)。

1. 在 Releases 的 Assets 中下载 macOS Apple Silicon 的 `.dmg` 安装包。
2. 打开 DMG，将 `QuotaHorizon.app` 拖入旁边的 Applications 文件夹。
3. 推出磁盘映像，从“应用程序”打开 QuotaHorizon。
4. 按界面提示选择本机已登录的 Codex 安装，读取当前账号的额度。

若 macOS 阻止首次打开，在“系统设置 → 隐私与安全性”中找到 QuotaHorizon，点击“仍要打开”，
再按系统提示确认。详见 [macOS 打开应用说明](https://support.apple.com/zh-cn/102445)。

应用常驻菜单栏。点击顶部额度打开弹窗，从弹窗进入主窗口；关闭主窗口后继续监控。
需要结束应用时，使用菜单栏中的退出命令。

## 更新

1. 从 Releases 下载新版 DMG。
2. 从菜单栏退出 QuotaHorizon。
3. 打开新版 DMG，将应用拖入 Applications，选择“替换”，然后重新打开。

账号、设置和历史保留在本机应用数据目录。macOS 可能在更新后重新请求历史密钥访问权限。

## 历史授权

额度规划显示“授权本地历史密钥”时，点击按钮并在 macOS 系统窗口完成授权。
授权成功后，应用继续读取原有历史。同一构建后续启动通常可直接访问。

系统密码只在 macOS 提示中输入。暂时取消授权时，实时额度仍可使用，历史记录留在本机。
若授权后历史仍为空，可查看页面中的账号、数据时间与诊断信息。

## 配置

可在设置中选择语言、刷新间隔、开机启动和规划实验室。
添加账号、设置日程及接入 Provider 的入口见[配置说明](CONFIGURATION.md)。
