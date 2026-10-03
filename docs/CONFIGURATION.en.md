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

The overview shows Horizon’s experimental 24 / 48-hour estimates, source values and community contributions side by side.
An active explicit announcement takes precedence at 100% for each covered window; the updates section tracks its timing, covered accounts and reported completion.
Original estimates from [Codex Reset](https://codex-reset.com/), [Codex Reset Monitor](https://codexreset.org/) and [NextReset](https://nextreset.ai/) remain visible.
Expand “Evidence & source review” to inspect weights, shared original posts, update times and reasons for excluding other sources. Stale or failed reviewed sources stay visible and are excluded from the combined estimate.

One chart uses colors for Horizon and each source, solid lines for 24h, and dashed lines for 48h. The x-axis uses forecast generation time, and the legend shows each series’ first record; repeated reads retain one point. The chart initially fits the local recording period, with a full 24-hour view including Monitor’s earlier history available. Hover for generation time, local collection time and probability.
The community panel shows the past 24 hours of public GitHub and Hacker News samples, including replies in active reset discussions, and separates predictions, wishes and reported experiences.
Both automatic-reset and credit-issuance expectations inform sentiment. Unexpired, explicit predictions contribute after author, duplicate-text, shared-evidence and age adjustments. Each horizon shows its effective sample, contribution and trend; “Views, evidence and sampling” lists statements and collection status.
Quantified community outlook also shows Codex Reset’s timing-vote distribution and NextReset’s daily probability-poll means and past rounds. Charts retain the original deadlines, vote counts and sources. Votes inform only horizons with an identifiable direction, and anonymous ballots share a contribution cap.
WeChat, Reddit and Linux.do are not connected. Missing directional predictions appear as `—`.
Tibo’s lead statement is fully visible by default, with English and available translations together in the Chinese interface. “Other posts and replies” includes unrelated activity: filter replies, open full text and available parent context, or visit X for more interactions.
“How the estimate is calculated” explains the current experimental weights and community adjustment.
Event history combines records from Codex Reset and [QuotaResets](https://quotaresets.com/); service updates come from [OpenAI Status](https://status.openai.com/).

Reset history uses a three-month calendar to distinguish quota resets, reset-card grants and announcements. Select a date to see event times in your computer's timezone.
A small dot marks new content, which is marked read when viewed; “Mark all read” is also available. Each source's name and update time accompany its content, and failed connections retain the previous result.
