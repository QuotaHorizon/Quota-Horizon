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
| Reset radar | Check primary news and forecast sources about once a minute; manual refresh updates evidence and predictions while historical evaluation runs in the background |

Accounts and history are stored locally. Viewing multiple accounts' quota keeps the active login in place.
See [CPA bridge](CPA_BRIDGE.en.md) for environment variables and response formats, and [installation](INSTALLATION.en.md#history-access) for history access.

## Reset radar

The overview shows the next 24 / 48-hour outlook side by side. An active explicit announcement takes
precedence at 100% for each covered window, while retaining model estimates, scheduled times and subsequent completion reports.

The macOS and Windows x64 installers include the [independent forecasting and evaluation modules](DEVELOPMENT.en.md#connect-the-desktop-radar).
Opening the app starts background collection; no separate Python installation is needed.
The main estimate covers a reset or a newly issued reset credit, with automatic-reset probabilities alongside it.
The next-72-hour chart compares the combined estimate, community aggregate and historical baseline. Source comparisons
retain each radar’s published values, event scope and issue time. Expand model inputs to see aligned 24h / 48h values
and each source’s supported horizon. Sources awaiting quality or scope review remain in the details. Expired results
retain their previous values and timestamps.

The community panel shows its aggregate probability, actual group weight, recent votes and when they were detected, and the percentage-point difference between
the full model and its community-removed counterpart. Probability inputs require at least 50 new ballots in the past 6 hours;
collection continues for polls below that threshold. Original community ballots preserve Codex Reset’s timing
histogram and NextReset’s daily probability votes, deadlines and counts. The history chart shows Horizon and all
sources together: solid lines for 24h, dashed for 48h. It records on a shared clock from the evaluator’s actual receipt
of forecasts and leaves missing windows blank. Hover for time and probability.

“Why the outlook changed” compares forecasts actually issued by the same model revision, with 24h / 48h changes
and contributions from sources, community inputs and event-scope adjustments. Expand it for before-and-after
inputs, weights and published source values. New revisions first accumulate a comparison.

“Do the predictions hold up?” shows frozen records, settlement progress and evidence of actual resets and credit grants.
After windows mature with sufficient outcome coverage, Brier scores, log loss, calibration, alert quality and matched
comparisons appear automatically; lower loss scores are better. A new evaluator first shows collection progress.
Corrections to event evidence update the settlement.
Evidenced events with monitoring gaps receive separate error checks; main scores and calibration use fully monitored windows.
Expand the score count to inspect settled windows, headline scores and rolling checks for 6h / 12h / 24h / 48h.
Sources are scored separately on shared windows; score counts and actual reset-event counts are shown separately.
Mature windows with gaps or conflicts in evidence show as awaiting sufficient evidence.
Horizon periodically checks independent processes and restarts exited workers while the app runs.
After a computer restart, opening Horizon restores collection.

Installations without a connected independent module continue using the built-in radar, which combines Codex Reset,
Reset Monitor, NextReset, public discussions and polls, with source status and sampling details.
Tibo’s lead statement is fully visible; the Chinese interface includes the original English and available translation.
“Other posts and replies” includes unrelated activity: filter replies, open full text and available context, or visit X for more interactions.
Conditional commitments such as a daily improvement or full reset stay prominent during their stated period, with the conditions and duration preserved.

Reset history uses a three-month calendar to distinguish quota resets, reset-card grants and announcements. Select a date to see event times in your computer’s timezone.
A small dot marks new content, which is marked read when viewed; “Mark all read” is also available. Each source’s name and update time accompany its content, and failed connections retain the previous result.
