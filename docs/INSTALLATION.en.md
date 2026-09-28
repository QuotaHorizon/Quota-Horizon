# Installation and first run

[简体中文](INSTALLATION.md) · **English**

For Apple Silicon (M-series) Macs running macOS 13 or later.

## Install

The installer is available on [GitHub Releases](https://github.com/QuotaHorizon/Quota-Horizon/releases).

1. Download the macOS Apple Silicon `.dmg` installer from the release's Assets section.
2. Open the DMG and drag `QuotaHorizon.app` into the adjacent Applications folder.
3. Eject the disk image and open QuotaHorizon from Applications.
4. Follow the prompt to select your signed-in local Codex installation and read the current account's quota.

If macOS blocks the first launch, find QuotaHorizon under System Settings → Privacy & Security, click Open Anyway,
and follow the system prompt. See [opening apps on macOS](https://support.apple.com/en-us/102445).

The application stays in the menu bar. Click the quota to open the popover and access the main window; monitoring continues when the window closes.
Use the menu-bar quit command to stop the application.

## Update

1. Download the new DMG from Releases.
2. Quit QuotaHorizon from the menu bar.
3. Open the new DMG, drag the app into Applications, choose Replace, then open the app again.

Accounts, settings and history stay in the local application data directory. macOS may request history-key access again after an update.

## History access

When the quota planning page shows the history-key authorization button, click it and complete the macOS system prompt.
Once authorized, the application continues reading existing history. Later launches of the same build can usually access it directly.

Enter your system password only in the macOS prompt. If you cancel for now, live quota remains available and history stays on the device.
If history is still empty after authorization, check the account, data timestamp and diagnostics shown on the page.

## Configuration

Settings include language, refresh interval, launch at login and the planning lab.
See [configuration](CONFIGURATION.en.md) for accounts, schedules and Provider connections.
