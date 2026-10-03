# Configuration

[简体中文](CONFIGURATION.md) · **English**

| Feature | Setting |
| --- | --- |
| Language | Follow system, Chinese or English |
| Quota refresh | Manual, 1/5/15 minutes; failed refreshes retain the previous reading and capture time |
| Codex source | Select a local reader; confirm again when its location or file identity changes |
| Accounts | Add, renew and view accounts; confirm changes to the active login |
| Work schedules | Set working and unavailable hours to allocate the current quota |
| Planning lab | Off by default; enable recent usage projections and outcome records |
| History | Stored per account; complete system authorization when first needed |
| Providers/API | Configure service addresses, models and credentials, then preview and apply changes |
| CPA quota pool | Off by default; configure an SSH destination and query command to enable it |
| Reset radar | Collect public sources every 15 minutes in the background, with manual refresh available |

Accounts and history are stored locally. Viewing multiple accounts' quota keeps the active login in place.
See [CPA bridge](CPA_BRIDGE.en.md) for environment variables and response formats, and [installation](INSTALLATION.en.md#history-access) for history access.

## Reset radar

The overview shows Horizon’s experimental combined estimate, forecast trends, source comparisons and community outlook. Select 24 / 48 hours to change the forecast window.
An active explicit announcement takes precedence at 100% for each covered window; the updates section tracks its timing, covered accounts and reported completion.
Original estimates from [Codex Reset](https://codex-reset.com/), [Codex Reset Monitor](https://codexreset.org/),
[QuotaCue](https://quotacue.com/) and [CodexReset.app](https://codexreset.app/) remain visible.
Expand “Evidence & overlap” to inspect weights, shared original posts, source generation times and local collection times. Stale, failed or reset-inconsistent sources stay visible and are excluded from the combined estimate.

The Horizon trend accumulates from the first local observation. You can also select the historical forecasts recorded by Codex Reset Monitor.
The community panel samples public GitHub and Hacker News content, deduplicates authors, and separates predictions, wishes and reported experiences.
Enough independent predictions with explicit horizons can adjust the combined estimate. “Views, evidence and sampling” shows statements, sample counts and collection status.
WeChat, Reddit and Linux.do are not connected. Missing prediction samples appear as `—`.
Expand statements in the updates section to read Tibo posts, translations and reply context. “How the estimate is calculated” explains the current weights and community adjustment.
Event history combines records from Codex Reset and [QuotaResets](https://quotaresets.com/); service updates come from [OpenAI Status](https://status.openai.com/).

Reset history uses a three-month calendar to distinguish quota resets, reset-card grants and announcements. Select a date to see event times in your computer's timezone.
A small dot marks new content, which is marked read when viewed; “Mark all read” is also available. Each source's name and update time accompany its content, and failed connections retain the previous result.
