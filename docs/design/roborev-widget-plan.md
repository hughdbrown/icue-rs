# Roborev widget — plan

Widget id `dev.edgehost.roborev`. Shows the roborev review queue, pass rate and the latest
verdicts from the local roborev daemon (https://github.com/kenn-io/roborev).

## Why a plain widget (no add-in, no Rust provider)

The roborev daemon serves a JSON API on a fixed loopback port, `127.0.0.1:7373` (its runtime file
`~/.roborev/runtime/daemon.<pid>.json` says so). Every read is an unauthenticated `GET`. A widget
`url` permission for `127.0.0.1:7373` routes `fetch()` through EdgeHost, so CORS doesn't matter.
`warthunder-hud` already works this way with port 8111. A Rhai add-in would only add a layer, and
a Rust provider is only needed for secrets or running commands, and this needs neither.

## Data sources (checked live 2026-10-17 against roborev v0.71.0)

| Endpoint | Used for | Notes |
|---|---|---|
| `GET /api/status` | queued / running jobs, active vs. max workers, `queue_paused` | Small. Poll every 5 s. |
| `GET /api/summary?since=<ISO>` | `verdicts.pass_rate`, `duration.review_p50/p90_secs`, `cost.total_usd`, per-repo and per-agent pass/fail | The default window is 7 days. Poll every 60 s. |
| `GET /api/jobs?limit=8&omit_prompt=true` | latest jobs: `status`, `verdict` (`P`/`F`), `repo_name`, `commit_subject`, `branch`, `enqueued_at`/`finished_at`, `agent` | **Always pass `omit_prompt=true`**: without it, 10 jobs are 640 KB because each job carries its prompt. With it, 10 jobs are 13 KB. |
| `GET /api/jobs?closed=false&limit=50&omit_prompt=true` | open failing reviews (verdict `F`, not closed) | No verdict filter exists, so count on the client. |
| `GET /api/health` | `healthy`, `error_count_24h` | Drives the status dot. |

The full list is at `GET /openapi.json`. `GET /api/stream/events` (SSE) could make updates live
later, but a routed `fetch()` probably doesn't stream, so v1 polls.

## Layout per slot

- **S (840×344)**: a large 7-day pass-rate ring. Next to it: queued and running counts, the count
  of open failing reviews (red when above 0), and a status dot (green healthy, amber paused, red
  daemon down).
- **M (840×696)**: S across the top, then the 8 latest jobs. Each row shows a verdict chip, repo,
  commit subject (truncated) and age. A running job shows elapsed time and a spinner.
- **L / XL**: M, plus per-repo pass/fail bars, p50/p90 review time, 7-day cost, and a per-agent
  table. XL adds a 14-day daily pass-rate sparkline built from `/api/summary` calls with sliding
  `since` values (cache them; refresh the past days hourly).
- Portrait: the same blocks stacked.

## Settings (manifest properties)

| Setting | Type | Default |
|---|---|---|
| Repository filter | text | empty = all repos (passed as `repo=` to summary and jobs) |
| Window | combobox 24h / 7d / 30d | 7d |
| Text, Accent, Pass, Fail, Background colours | color | house palette |

The port can't be a setting, because a `url` permission names one fixed port. 7373 is roborev's
default. If someone changes it, they edit the manifest.

## States

- Daemon down (fetch fails): grey everything out, show "roborev daemon not running —
  `roborev daemon start`", and retry every 10 s.
- No jobs in the window: an empty-state line instead of 0%.
- `queue_paused`: amber banner "queue paused".

## Steps (each a commit; `just ci` green before each)

1. Scaffold `examples/widgets/roborev/` (manifest with the url permission, index.html,
   scripts/, styles/, translation.json, icon). `edgectl validate` passes.
2. A pure model module (`scripts/model.js`): turns status, summary and jobs JSON into a view
   model (pass rate, open-fail count, rows, ages). Write `roborev.test.mjs` against fixtures
   captured from the live daemon (prompt fields stripped, repo paths anonymised).
3. Rendering for S/M/L/XL and portrait, plus the polling loop with separate cadences and
   backoff when the daemon is down.
4. Settings wiring and the empty, paused and down states.
5. README.md and screenshot.png (per the widget README convention), the justfile JS test list,
   and CI.

## Open questions

- Should tapping a job row open its `web_url` (`http://127.0.0.1:50051/reviews/<id>`)? That
  depends on EdgeHost letting a widget open a URL on the Mac; check the SPEC before promising it.
- Check that `closed=false` really filters, and doesn't just return everything.
