# Forge widget — plan

Widget id `dev.edgehost.forge`. A maintainer-at-a-glance panel for kenn-forge
(https://github.com/kenn-io/forge): open PRs with CI and review state, live agent workspaces, and
sync health.

## Prerequisite: upgrade middleman to kenn-forge

The installed binary is forge's old name: `~/.local/bin/middleman`, built 2026-04-01, with config at
`~/.config/middleman/config.toml`. It uses **port 8090**, watches 3 repos, and **isn't running**.
Its embedded routes show only `/api/v1/version`, `/api/v1/repos` and `/api/v1/settings`.
Current kenn-forge serves a much larger API. It defaults to **port 8091** and keeps its config at
`~/.kenn/forge/config.toml`. The plan targets current kenn-forge, so step 0 is installing it and
moving the three `[[repos]]` entries over. Check upstream for a migration note first.

## Why a plain widget

Like roborev, kenn-forge serves JSON on a fixed loopback port (`/api/v1` on `127.0.0.1:8091`). API
auth (`[api].require_auth`) is off by default, so a `url` permission plus `fetch()` is enough. If
the owner turns auth on, the widget would need a bearer token. Header pass-through on routed fetch
hasn't been checked, so that case would move to a small Rust provider that reads
`~/.kenn/forge/auth_token`. v1 assumes auth is off.

## Data sources (from upstream `frontend/openapi/openapi.yaml`, 2026-10-17; not yet checked live)

| Endpoint | Used for |
|---|---|
| `GET /api/v1/pulls?state=open&limit=20` | rows: `repo`, `Number`, `Title`, `IsDraft`, `CIStatus`, `ReviewDecision`, `MergeableState`, `LastActivityAt`, `Additions`/`Deletions`, `worktree_links` |
| `GET /api/v1/pulls?state=open&involves_me=true` | "needs me" count |
| `GET /api/v1/workspaces` | agent workspaces: `agent_state`, `associated_pr_number`, `commits_ahead/behind`, `mr_ci_status`, `kata` link |
| `GET /api/v1/sync/status` | `running`, `last_run_at`, `last_error`, `progress` |
| `GET /api/v1/repos/summary` | per repo: `open_pr_count`, `open_issue_count`, `draft_pr_count`, `commits_since_release`, `latest_release`, `last_sync_error` |
| `GET /api/v1/notifications?state=unread&limit=1` | its `summary` gives the unread count |

`/pulls` rows include `Body`, so keep `limit` small. `/api/v1/version` serves as the liveness probe.

## Layout per slot

- **S**: three tiles. **Needs me** (PRs involving me), **Failing CI** (open PRs with
  `CIStatus` failure), and **Agents** (workspaces whose `agent_state` is active), plus a sync dot
  showing the last sync age.
- **M**: open PRs, up to 8 rows. Each row shows a CI dot, a review-decision chip (approved /
  changes / pending), `repo#n`, title, draft tag, age, and a worktree icon when `worktree_links`
  isn't empty.
- **L / XL**: M, plus a workspaces column (branch, agent state, ahead/behind, linked PR) and a
  repo strip (open PRs and issues, commits since the last release). XL adds the unread
  notification count and recent sync errors.

## Settings

| Setting | Type | Default |
|---|---|---|
| Repository filter | text `owner/name` | empty = all |
| Only PRs involving me | switch | off |
| Colours | color | house palette |

## States

- Daemon down: "kenn-forge isn't running — `kenn-forge daemon start`".
- `sync/status.last_error` set: an amber banner with the error code. GitHub rate-limit errors show
  their reset time (`last_error_ceiling_reset_at`).

## Steps (each a commit; `just ci` green before each)

0. **Owner**: install kenn-forge, move the repos over, `kenn-forge daemon start`. Then capture live
   fixtures with `curl` and confirm that the field names match the OpenAPI document (the `/pulls`
   rows mix PascalCase and snake_case).
1. Scaffold `examples/widgets/forge/` with a `url` permission for `127.0.0.1:8091`.
2. A pure model plus `forge.test.mjs`, using the captured fixtures with bodies stripped.
3. Rendering for all slots and portrait, plus polling (pulls and workspaces every 30 s, sync status
   every 10 s, repo summary every 5 min).
4. Settings and states.
5. README, screenshot, justfile JS test list, CI.

## Later

forge already reads roborev reviews and links kata issues (`/pulls/.../kata-links`,
workspace `kata`). A combined "Kenn" XL widget could show, for each PR, its CI state, its roborev
verdict and its kata issue in one row. Build it after the three single widgets are done.
