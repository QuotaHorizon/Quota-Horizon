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

The overview leads with explicit dated reset announcements, covered accounts and local time. An active announcement sets each covered 24/48-hour window to 100%.
Original estimates from [Codex Reset](https://codex-reset.com/) appear as bars under “How sources compare”. Select 24h / 48h to change the forecast window.
The progress diagram shows the announcement, scheduled time and reported completion; after the scheduled time, the outlook shows “awaiting confirmation”. Source failures, timing conflicts and corrections have their own states.
Related statements include Tibo posts, translations and reply context. Select a record in “Key updates” to expand it.
Event history combines records from Codex Reset and [QuotaResets](https://quotaresets.com/); service updates come from [OpenAI Status](https://status.openai.com/).

Reset history uses a three-month calendar to distinguish quota resets, reset-card grants and announcements. Select a date to see event times in your computer's timezone.
A small dot marks new content, which is marked read when viewed; “Mark all read” is also available. Each source's name and update time accompany its content, and failed connections retain the previous result.
