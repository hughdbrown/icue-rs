# Kata widget — plan

Widget id `dev.edgehost.kata`. Shows what's ready to work on, open-issue load per project and
recent throughput from the local kata daemon (https://github.com/kenn-io/kata).

## The one prerequisite: a TCP listener

By default kata v0.18.0 listens **only on a Unix socket**
(`$TMPDIR/kata-501/<hash>/daemon.sock`). A widget can't reach a socket. kata's web UI also
binds a loopback TCP port, but:
- the port changes on each start (`web_origin_stable: false`), and
- its `/api/v1/*` routes need a browser session (`401 web_session_required` for anything beyond
  `/health`).

`kata daemon start --listen host:port` (or `listen = "127.0.0.1:7474"` in `~/.kata/config.toml`)
binds the API to a fixed TCP port. The help text implies tokenless GETs are allowed on
loopback, because `--insecure-readonly` exists only for *non*-loopback addresses.
**Step 0 checks that before anything else gets built.** It's a change to the owner's daemon config,
so the owner makes it, not Claude.

If loopback GETs turn out to need a token, the fallback is a small built-in Rust provider
(`KataProvider`) that talks to the Unix socket directly. That's the same route as `LlmUsage`, and it
also keeps any token out of the widget.

## Data sources (checked live 2026-10-17 over the socket)

| Endpoint | Used for | Notes |
|---|---|---|
| `GET /api/v1/ready` | open issues with no open blocker: `title`, `qualified_id` (`channel-atlas#w7n9`), `project_name`, `created_at`, priority | This is the core of the widget. Each issue includes its `body`, so cap the list in the UI. |
| `GET /api/v1/issues?status=open&limit=N&sort=…` | open count per project | Also takes `project_id`, `priority` and `max_priority`. |
| `GET /api/v1/projects` | project names and ids for the filter setting | |
| `GET /api/v1/digest?since=<ISO>` | created / closed / commented / unblocked counts per actor (human vs. agent) | `since` is required. |
| `GET /api/v1/health` | up/down, version | |

`/api/v1/events/stream` (SSE) could replace polling later. The full schema comes from
`kata openapi`.

## Layout per slot

- **S**: a large "ready" count, the open count, and the top ready issue (`kata next` in effect):
  title plus `project#id`.
- **M**: the ready queue, up to 7 rows by priority. Each row shows a priority pip, `project#id`,
  title, age, and a deadline badge when one is set.
- **L / XL**: M, plus open issues per project (horizontal bars) and a 7-day digest: closed vs.
  created per day, split by actor. Agent actors are the ones whose author isn't the owner, which
  shows how much work the agents are closing. XL adds a 30-day closed-per-day sparkline.

## Settings

| Setting | Type | Default |
|---|---|---|
| Project | text (name) | empty = all projects |
| Digest window | combobox 1d / 7d / 30d | 7d |
| Owner name (to split human vs. agent in the digest) | text | empty = no split |
| Colours | color | house palette |

## States

- Daemon not reachable on TCP: show "kata isn't listening on 127.0.0.1:7474 — add `listen` to
  `~/.kata/config.toml` and `kata daemon restart`".
- Nothing ready: "Nothing ready. Blocked: N" (open minus ready).

## Steps (each a commit; `just ci` green before each)

0. **Spike, no code**: the owner adds `listen`. Check from `curl` that `GET /api/v1/ready` works
   with no token. Record the result here. If it fails, switch to the Rust-provider fallback above.
1. Scaffold `examples/widgets/kata/` with a `url` permission for `127.0.0.1:7474`.
2. A pure model plus `kata.test.mjs`, using fixtures captured from the live daemon (bodies
   stripped).
3. Rendering for all slots and portrait, plus polling (ready and health every 15 s, digest every
   5 min).
4. Settings and states.
5. README (including the `listen` setup), screenshot, justfile JS test list, CI.

## Open questions

- Port 7474 is a suggestion. Make sure it doesn't clash with roborev's 7373 or anything else
  the owner runs.
- Should the ready list include issues from all projects, or default to the project for the
  repo in the git widget's setting? Keep it independent for v1.
