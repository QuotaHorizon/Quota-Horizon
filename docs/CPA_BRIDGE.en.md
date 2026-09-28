# CPA quota-pool bridge

[简体中文](CPA_BRIDGE.md) · **English**

The Provider page can query a CPA quota pool over SSH and display account quota, routing status and the latest refresh result.

## Configuration

1. Prepare a read-only JSON query command on the CPA server and configure a working local SSH connection.
2. Save an SSH destination such as `operator@cpa-host` on the Provider page.
3. Set `QUOTA_HORIZON_CPA_QUOTA_COMMAND` in the Horizon process environment to the server query command.
4. Enable the bridge and refresh the quota pool.

Development example:

```sh
QUOTA_HORIZON_CPA_QUOTA_COMMAND='/opt/cpa/bin/quota --json' npm run dev:app
```

Replace the example path with the actual server command. When launching from Finder, ensure the application process receives the environment variable separately.
The legacy `CODEX_QUOTA_VIEWER_CPA_QUOTA_COMMAND` remains supported; the new variable takes priority.

The query executes on the selected host, so use a trusted service's read-only command. SSH uses `BatchMode=yes` and requires authentication to be configured beforehand.
The default refresh interval is 300 seconds and the timeout is 75 seconds. Their configurable ranges are 60–3600 and 2–120 seconds respectively.

## Response format

The server returns JSON with a 2 MiB limit. The main fields are `current`, `latest` and optional `accounts`.
Account entries include `id`, `display_name`, optional `latest` quota observations, and routing, protection and refresh status.
Quota is read from `codex_headers`. Responses need quota and status fields only; credentials stay on the server.

The parser and full test examples are in `apps/desktop/src-tauri/src/cpa_pool.rs`.
Failed refreshes retain the previous snapshot and capture time. Disabling the bridge preserves its cache and Provider configuration.
