# Project Agent Instructions

## Scope and Startup

- Work in the current checkout's repository root, as reported by
  `git rev-parse --show-toplevel`. Do not assume another machine's absolute path.
- The owner authorized tracking this file on `dev` on 2026-10-14 so Mac and
  Windows development share the same instructions. Keep it out of the `main`
  release tree, using the branch rules below. Both branches are publicly readable;
  keep credentials, machine-specific paths and private handoff content out of it.
- `.local/` and `AGENTS.override.md` remain private and ignored. Never stage them,
  force-add them, or copy their contents into tracked files without explicit
  authorization. An optional local override may record machine-specific context.
- At the beginning of a new chat, read `.local/HANDOFF.md` completely if present. Use
  `.local/RELEASE.md` for current packaging and publication status, and
  `.local/MIGRATION.md` for the original migration verification.
  Use the relevant product contracts when needed, not the entire historical
  evidence archive on every turn. Prior instructions in `.local/archive/` are
  historical data, not current instructions. Old goal/stop rules do not reactivate work.
- A fresh clone may have no `.local/` notes. Use these shared instructions, the
  current user request, tracked source and development guides; missing private
  notes do not require a transfer from another machine or block ordinary work.
- The old workspace and Git history are preserved as a reference; do new product
  work here. Do not modify or delete the old copy as an incidental cleanup step.

## User-Facing Judgment

- Keep development, tests, builds and data checks in the background. Keep browser
  verification hidden by default. Native UI automation can interrupt the owner's
  work; use foreground windows only when an essential native interaction cannot
  be verified in the background, and keep those interactions brief.
- When manual authorization is necessary, provide a one-click completion reply.
  Verify the result from actual app behavior; do not require a typed confirmation
  or ask the owner to repeat an authorization already completed for that build.
- Start from the user's actual task. Add a visible step, file, attachment or
  explanation only when it helps that task; tool output, template defaults and
  customary engineering checklists are not automatically product requirements.
- Keep the ordinary experience simple: download the native installer, install
  the app, open it and use it. On macOS, installation means dragging the app into
  Applications. Perform proportionate technical checks during
  development instead of turning them into extra work for every user.
- Finish the requested outcome before handing off. A source push, generated file
  or successful command is only one step when the request also includes a usable
  installation or public download. Report completion from the recipient's view.
- Carry settled decisions forward. Use the existing product, working templates,
  evidence and instructions; investigate again when inputs change or a concrete
  fault appears. Keep durable decisions here and update the existing short notes.

## Product Priorities

- The owner confirmed on 2026-10-02 that QuotaHorizon has fully replaced the
  customized QuotaViewer as an independent, reliable daily-driver tool. Optimize
  QuotaHorizon itself while preserving existing workflows. Retain
  `.local/public-text-before-20260928/docs/QUOTAVIEWER_PARITY.md` as historical
  requirements, not an unfinished replacement gate or the current roadmap.
- Continue the existing Tauri/Rust/React product based on Codex Switch v1.3.5.
  Reuse working modules and the owner's QuotaViewer assets. Do not restart from
  a blank skeleton or repeat completed research. Preserve existing provenance
  notices; do not reopen settled source/license discussions as routine blockers.
- Prioritize observable daily-driver improvements: menu-bar quota, reliable
  automatic refresh, consistent windows, accounts, history, daily schedule,
  sessions and reset radar. Necessary correctness and data protection remain;
  avoid speculative defensive architecture and endless acceptance scaffolding.
- Preserve user outcomes, not the old visual layout. Improve appearance when it
  helps usability. Do not put unfinished experiments ahead of working features.
- Keep the experimental planning lab closed by default and its background trials
  paused. Do not present schedule budgets or uncalibrated estimates as validated
  work-capacity predictions. The full planning ambition remains a later task.
- UI text should lead with results, time, source and an action when needed.
  Avoid repetitive disclaimers, internal phase IDs and raw reason/status enums.
  Keep real missing/stale/error states and consequential-action warnings.
- Public installers are generated only from an accepted `main` commit for an
  external release explicitly confirmed by the owner. Source publication alone
  does not complete an authorized installer release. The 2026-10-14 requirement
  to deliver macOS Apple Silicon and Windows x64 from the same source and version
  applies to that release workflow, not routine `dev` work. Preserve both platforms'
  working behavior; run release CI only within its separately authorized scope.
  Daily development uses a local build and direct replacement on the current
  machine, without Windows builds, CI, or public installer generation unless
  the owner specifically requests them.
  The owner has explicitly excluded Apple notarization and paid enrollment from
  this plan. Do not reopen these as blockers, recurring recommendations or tasks.
- For an authorized public release, reuse established installer templates.
  The macOS DMG uses Sindre Sorhus's create-dmg template. Validate the real Finder
  window and provide its screenshot; a structurally valid image alone is not
  complete installer UX. Keep packaging dependencies local to the project.

## Git and Delivery

- On 2026-10-16 the owner granted standing authorization for local `dev` commits.
  After completing and verifying a coherent work package, create its commit
  without asking for separate commit approval. Review the exact diff and stage
  only that work package; preserve unrelated changes and exclude private notes.
  This authorization does not include pushing, merging into `main`, tags,
  GitHub CI, or public releases.
- Routine `dev` app delivery is local: run proportionate checks, build for the
  current machine, directly replace the daily-use app, reopen it and verify the
  affected behavior and data continuity, then commit the completed work package.
  Do not turn routine delivery into Windows validation, CI, DMG/EXE packaging,
  or release preparation. Public installers belong only to an owner-confirmed
  release of an accepted `main` commit; local app bundles are development builds.
- Keep `AGENTS.md` tracked on `dev` and development branches, and absent from the
  committed `main` tree. `.gitignore` only protects untracked copies; it does not
  exclude a tracked file from a merge.
- For an authorized release into `main`, prepare a temporary `codex/release-*`
  branch from the accepted `dev` commit, remove `AGENTS.md` on that candidate
  branch, and open the release PR from there. Verify that
  `git ls-tree --name-only <candidate> -- AGENTS.md` is empty before merging.
  Keep the file on `dev`; preserve it when bringing release fixes back from
  `main`. Never merge the unfiltered `dev` tree directly into `main`.
- `npm run check:source` rejects a tracked `AGENTS.md` on local `main`, a push to
  `main`, or a PR targeting `main`. Run this check on the release candidate;
  authorization is still required before any GitHub CI run or publishing action.
- Every GitHub CI run requires the owner's explicit authorization for its scope.
  This includes pushes and pull requests that trigger workflows, manual dispatches,
  and reruns of failed jobs. Source-push permission alone does not authorize CI.
  Do not expand a Mac-only authorization to Windows or retry a failed run without
  authorization. Read-only inspection of existing run status and logs is allowed.
- Use one organization-owned product repository for ongoing development. Do not
  import the old parent Git history or maintain a private/public dual-repository
  synchronization workflow. Keep private operations and handoffs outside public source.
- Use the owner's chosen branch names: `main` for stable code and `dev` for
  ongoing development. Keep branch names concise and conventional. Identify
  delivered versions with tags/releases and record ordinary fixes and reverts honestly.
- The initial independent baseline is published on `main` in
  `https://github.com/QuotaHorizon/Quota-Horizon`. Continue work on `dev`, validate
  and commit the natural work package locally. Only when the owner confirms an
  external release, prepare the filtered release pull request into `main`, tag
  the accepted commit and match public installers to that exact source. Push,
  merge, CI and release authorization remain separate from standing local `dev`
  commit authorization. Keep these conventions in this development-branch file.
- Folding the initial publication corrections into one commit was explicitly
  authorized for that initial release. Future published-history rewrites require
  their own authorization; normal development retains meaningful commits.

- All commit titles and bullet bodies must be written in English. A commit message
  must have a short title, one blank line, then meaningful hyphen-prefixed bullets
  describing the delivered changes. Example:

  ```text
  Improve reset radar reading flow

  - Focus the overview on probability and related statements
  - Preserve history in a separate view and test read-state behavior
  ```

- Never add AI co-author/coworker trailers, AI-generated attribution, bot
  signatures, or `Co-Authored-By` attribution to an AI in commits or PRs.
- On 2026-10-16 the owner explicitly authorized automatic removal of any
  tool-added AI attribution from local, unpublished commits created for the
  current work. Do not ask for separate signature-removal approval again.
  Amend only the affected commit message, preserving its tree, parents and
  author identity; verify the actual stored message after creation or correction.
  This does not authorize rewriting published commits or changing Git identity,
  configuration or hooks.
- Commit only reviewed, in-scope changes under the standing local `dev`
  authorization or a specific user request. Never stage the entire workspace
  blindly; confirm the exact staged paths and that local handoff files remain
  excluded. Do not force-add ignored files.
- This repository uses the user's confirmed GitHub noreply email for commits.
  Preserve global Git settings. Account-settings access was questioned by the
  user; continue with the confirmed local identity and obtain explicit permission
  before any future account-settings inspection.
- The user authorized the initial commit/push and the project chat's development
  branch setup. Future publishing actions follow the current user request;
  GitHub Release creation and installer upload remain separate from source pushes.
- For routine app changes, complete the local build/install/verification sequence
  above before committing. Source completion alone is not installation completion.
  Respect any task-specific restriction and report a genuine remaining step plainly.
- The owner requested direct app replacement on 2026-10-03 and removal of all
  previous QuotaHorizon backups on 2026-10-12. Do not create or retain routine
  app, data or source backup copies unless the owner explicitly requests them.
  Preserve live data in place and verify continuity proportionately. The earlier
  instruction to retain historical recovery points is superseded.
- On macOS, after a build has completed and its required installation or packaging checks
  have passed, delete generated `.app` bundles from build directories and any
  temporary installation or packaging copies. Keep `/Applications/QuotaHorizon.app`
  as the daily-use application. Do not leave duplicate launchable apps in search.
  Keep source, compiler caches and intended installer deliverables; cleanup must
  not discard an artifact before its requested installation or packaging is done.
- During long builds, keep build inputs frozen and use the time for independent
  analysis/docs/review. Do not idle unnecessarily or introduce another long soak
  for each small fix. Respect explicit compute/build restrictions.
- Report what changed, what was actually tested, what is installed, and what is
  not yet verified. Do not inflate test counts into a claim that 1.0 is complete.
- Reuse completed validation for unchanged installer artifacts. Release-only
  work needs upload and download verification; repeat builds, architecture checks
  or signing checks only when relevant inputs changed or an actual fault warrants them.

## Public Documentation and Copy

- Include a public document only when it helps someone use, install, configure or
  build the current product. Identify that concrete task before adding a file or
  section; prefer an existing guide. A conventional filename is not a reason to
  publish it. Keep roadmaps, internal architecture references, experimental
  algorithm notes, implementation research and review records in `.local/`.
- Review publishable text as a first-time user with no development history:
  what does this app do, how do I install and use it, and why does this sentence
  help me? Apply this lens to README, UI copy, release notes and commit summaries.
  Describe delivered user outcomes instead of implementation effort or process.
  Reserve developer details for the essential source-build guide. Lead with the
  feature, result and next action, using short, direct, affirmative text.
- Avoid defensive framing and repeated disclaimers such as "not X", "does not
  mean", "cannot be claimed", "not a guarantee", and their Chinese equivalents.
  Describe scope and behavior directly. Keep concrete data-loss, security and cost
  warnings at the action that needs them, and state actual platform availability.
- Public development guides cover dependencies, startup, builds, tests and source
  layout. Personal review preferences, bilingual-maintenance instructions, commit
  formats, branch conventions, assistant instructions, authorization history,
  work-package diaries, test-count reports and acceptance gates belong in `.local/`
  or this development-branch instruction file, as appropriate. Keep them out of
  product-facing documentation and the `main` release tree.
- Keep Chinese and English user documentation equivalent and linked, without
  explaining the owner's review language or the assistant's writing process.
- Use the repository's stable Releases page for download links in README and
  installation guides. Keep release-specific versions and installer filenames on
  release pages so routine releases do not require rewriting the user guides.
- Publish the macOS DMG as the macOS installer asset. Keep `SHA256SUMS`, checksum sidecars,
  manual hash-verification commands and maintenance evidence outside the public
  download flow unless the user explicitly requests them. Local validation can
  use checksums. User guides focus on download, installation and use; GitHub's
  own generated asset metadata and source archives remain platform-provided.
- Keep UI messages about the current state and useful actions. Missing data and
  consequential-operation warnings remain explicit; technical diagnostics belong
  in their appropriate detail views rather than repeated banner disclaimers.
- Document the current implementation. Verify source behavior before describing
  refresh cadence, supported capabilities or default settings. Screenshot captions
  describe what is visible, with synthetic examples identified once.
- Preserve license/attribution files. Retain technical references and product
  requirements in `.local/` when they are unnecessary for the public guides;
  editorial cleanup never lowers scope. Public links must resolve within the
  published source set or to a relevant public site, not to ignored local files.
- Keep public DMG build instructions in `docs/DEVELOPMENT.md` and its English
  counterpart. Record current artifact hashes, release preparation, publication
  steps and verification evidence in `.local/RELEASE.md`. Document the DMG installer
  format; auxiliary maintainer files stay outside the public download workflow.

## Local Knowledge Maintenance

- Keep shared working principles in this file and machine-specific facts in
  ignored local notes. README serves product users.
- Maintain existing entry points: `HANDOFF.md` for current state and requirements,
  `RELEASE.md` for packaging and publication, and `MIGRATION.md` for original
  provenance. Keep each fact in one authoritative place and link to it elsewhere.
- Update the existing notes at a natural completion point. Routine fixes and
  checks need a concise result, not a new dated report, checklist or log bundle.
- Retain a technical reference only when it supports an active requirement,
  reusable implementation or unresolved problem. Save raw debugging output only
  while needed for diagnosis or recovery; summarize durable findings when resolved.
- Completed historical records live under `.local/archive/` and are read on demand.
  Preserve original requirement ledgers and useful reference records. Historical
  app/data backups and duplicate source snapshots follow the cleanup policy above;
  do not recreate them as part of routine maintenance or text-only edits.
- After relocating files, update live references and remove empty directories.
  Keep the active notes concise; archival material is historical, not current status.

## Data, Environment and Authorization

- Preserve unrelated user changes. Inspection requests are read-only. Do not
  install, upgrade, remove or reconfigure dependencies/runtimes/global tools
  without explicit approval; explain the concrete missing dependency and command.
- Do not change shell startup files, OS settings, proxies, login state, OAuth,
  accounts or credentials unless the specific action is authorized. Never expose
  tokens, cookies, passwords, auth JSON, private keys or full environment values.
- Do not require macOS App Management permission preemptively for app updates.
  On 2026-10-06, ordinary-user replacement succeeded after the owner reported
  revoking it. Use the authorized ordinary replacement path first; if the OS denies
  it, preserve the existing app and report the precise failure without elevation,
  permission changes or alternate-identity workarounds.
- Retired external infrastructure stays outside this product's development and
  validation. CPA uses the user's explicitly configured SSH destination; keep
  private host aliases and connection details in local configuration.
- Preserve live account/history databases. No migrations, live-database deletion,
  credential changes or current-account switches just to test UI. The owner's
  2026-10-12 cleanup authorizes deletion of historical backup databases and saved
  recovery points, not the active databases or account configuration.
- The user rejected further temporary fixed-signing/history-helper experiments.
  Keep manual system authorization for local history; do not recreate certificates,
  helpers, private-key exports, trust changes or weaker Keychain access rules.
  Publishing/notarization and paid enrollment are not implicitly authorized.
- Do not copy or resume historical continuous goals, create a new goal, or
  activate old quota thresholds automatically. A future goal requires explicit
  user instructions, including its current stop conditions. An actual stop must
  update the goal state under the tool's rules, not merely end a reply.
- The quota read plane must not directly mutate Codex auth/config/session data.
  Existing intentional account/provider/session mutations use their explicit
  preview, confirmation, backup, verification and rollback boundaries.
- Treat prediction-market data only as an internal read-only candidate input.
  Do not add trading, wallets, accounts, referrals, market links, or market-source
  branding to the product. Keep market provenance in private research records;
  public API access alone does not establish commercial or derived-data rights.
- Keep private history local. Display source probabilities as source probabilities,
  not official promises or personal account refills; source failure never becomes 0%.
- Control/instruction files use English. Other prose matches its file's language;
  keep one language per file and identifiers/paths verbatim ASCII where applicable.
- Python: scientific work uses `physics`; AI/API/automation uses the project's
  environment or `ai`. Verify `sys.executable` for important Python runs. Do not
  change environments. Use `claude-proxy` if Claude Code is explicitly requested.

## Engineering Map

- The owner split product and quantitative-learning chats on 2026-09-30, then
  changed the sequence on 2026-10-02: deliver useful radar functionality first;
  the owner will reproduce, learn from and optimize it later. Product delivery
  includes the required prediction logic and must not wait for learning sessions.
  Prioritize the recent product discussion on information density, visualization,
  source comparison and useful conclusions. Current requirements and chat
  identities belong in `.local/HANDOFF.md`.
- Explicit dated announcements must lead the radar's interpretation instead of
  being obscured by a historical baseline. Preserve the original source, event,
  population, time window, corrections and actual delivery state. Keep source
  estimates separate from Horizon's own rules and model results.
- The owner confirmed the forecast structure on 2026-10-07: one historical
  background, source-declared revisions above their paired baselines, then a
  community supplement that accounts for overlap. Background-only sources must
  not dilute news revisions as zero-effect votes. Do not score statement keywords
  or add an embedded language model to assign probabilities. Preserve signed
  revisions, source values and reproducible contributions; validate accuracy
  prospectively rather than targeting a desired headline percentage.
- Keep learning notes and exploratory research in `.local/`. Deliver reusable
  model logic through an explicit input/output contract for product integration.
  Coordinate shared-file edits before changing common types, storage, or runtime
  collectors; both chats may be using the same checkout.

- Preserve complete external radar estimates, including their upstream signal adjustments.
  The owner rejected home-grown keyword scoring or an embedded language model for assigning
  probability weights to statements. Use measured external forecasts and ballots; keep text
  available for reading, evidence and outcome verification.
- On 2026-10-11 the owner rejected selecting a message to gate numerical source weights.
  Message recency, references, display selection and announcement context must not exclude
  valid radar forecasts, change their weights or move the community sampling window.
  Use comparable complete source values with explicit, stable numerical aggregation rules.
  Collect separately published community/comment and commitment estimates as well as
  headline horizon values. Resolve their event/time scope from upstream structured data;
  fill only the information missing from the same source and retain one source contribution.
  Keep original values, supplemental contributions and matching provenance inspectable.

- On 2026-10-13 the owner confirmed a minimum of 50 newly observed ballots in the
  trailing six hours for community probability inputs. Cumulative totals cannot
  satisfy this threshold. Continue collecting smaller samples, but exclude them
  from probability calculations. Repeated clicks must remain click units, never
  unique participants. Evaluate mixed preference/prediction questions separately
  on their original event windows before admitting a probability contribution.

- Formal desktop: `apps/desktop/` (React/TypeScript/Tauri).
- Rust domain/runtime/storage/mutation: `crates/`.
- `apps/capacity-preview/src-tauri` and `apps/capacity-preview/src/status.ts` are
  production dependencies retained at their existing paths. The standalone preview
  UI has been retired. Preserve the service, types and isolated native regression.
- `.local/reference/ARCHITECTURE.md` describes modules and data flow. That directory
  also holds the roadmap, reset-source implementation, experimental algorithms and
  interface-check instructions. Public guides cover the active build and settings.
- Private decisions, the full parity ledger, planning contracts and historical
  acceptance records remain under `.local/public-text-before-20260928/docs/`.
  `.local/archive/2026-09-28/SHOWCASE-INSTALL-2026-09-28.md` records installed-app
  verification; `.local/HANDOFF.md` and `.local/RELEASE.md` hold current facts.
- Public packaging should contain the complete product dependency closure, not
  the old parent repository's operational code or personal history.
