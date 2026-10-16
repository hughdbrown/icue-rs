# LLM Usage widget — plan

Source prompt: [`llm-usage-prompt.md`](llm-usage-prompt.md). Goal: show every LLM subscription, how
much quota is left, when it resets, and whether you are on pace to use it all up before it resets.

## Decisions (owner, 2026-10-16)

| Question | Answer |
|---|---|
| Credentials | Hybrid: a 0600 TOML config lists subscriptions; each entry reuses a CLI login or carries its own key/cookie |
| Expired CLI OAuth tokens (Claude, Codex) | Refresh **and write back** to the CLI's store (Keychain / `auth.json`) |
| Claude token | Read from the macOS Keychain via `security` (`~/.claude/.credentials.json` elsewhere) |
| Gemini | Consumer AI Pro/Ultra → gemini.google.com usage page with a pasted Cookie header |
| Kimi, Ollama | API keys in the config; the owner pastes them for the live test |
| Slot | Designed for **M** (840×696); S and L lay out reasonably |
| Pace | Pace marker on each bar, ahead/behind colour, and a "use next" suggestion |

## Why not a Rhai add-in

Add-ins cannot send `Authorization`/`Cookie` headers, POST, or read files and the Keychain
(`docs/addin-api.md`). So the data side is a **built-in Rust provider**, like FileSystem.

## Data sources (researched 2026-10-16; all private/undocumented except Ollama and Moonshot)

| Provider | Request | Windows | Credential |
|---|---|---|---|
| Claude | `GET api.anthropic.com/api/oauth/usage`, `anthropic-beta: oauth-2025-04-20` | `five_hour`, `seven_day` (+ model-scoped weekly) — % used | Keychain `Claude Code-credentials` → `claudeAiOauth.accessToken`; refresh `POST platform.claude.com/v1/oauth/token` |
| Codex | `GET chatgpt.com/backend-api/wham/usage`, `ChatGPT-Account-Id` | `primary_window` (5 h), `secondary_window` (week) — % used, `reset_at` epoch s | `~/.codex/auth.json`; refresh `POST auth.openai.com/oauth/token` (JSON) |
| Gemini app | `GET gemini.google.com/_/BardChatUi/graphql/schemas/GEMINI_WEB_GRAPHQL/executeQuery?query_signature=…` with Cookie | `GXU_FIVE_HOURLY`, `GXU_WEEKLY` — fraction used, `windowRefreshesAt` | Cookie header pasted from the browser |
| Kimi Code | `GET api.kimi.com/coding/v1/usages` (Bearer) | old shape: `usage` (week) + `limits[]` (5 h); new shape: `limit_5h`, `limit_7d`, `limit_month_total` ratios | Kimi Code API key |
| Ollama Cloud | `GET ollama.com/api/balance` (Bearer, official) | credit plans: `included.balance_usd / allowance_usd`, `period.until`; legacy: `session`, `weekly` remaining % | Ollama API key |

Tokens rotate, so refresh is guarded: re-read the store just before refreshing (the CLI may already
have done it), refresh once, write the whole credential object back with only the token fields
changed, and serialise refreshes process-wide.

## Architecture

```
crates/llm-usage          pure library: config, credentials, HTTP fetchers, parsers, pace
  ├─ model.rs             Snapshot / Subscription / Window (serde → JSON for the widget)
  ├─ pace.rs              elapsed fraction, pace, projected run-out, "use next" ranking
  ├─ config.rs            TOML, 0600 check, default path
  ├─ http.rs              tiny Http trait (ureq impl, fake in tests); 429 Retry-After
  ├─ creds/{claude,codex}.rs   read / refresh / write back
  └─ providers/{claude,codex,gemini,kimi,ollama}.rs   fetch + parse (parsers fixture-tested)
crates/edge-plugins       built-in provider edgehost.llmusage:LlmUsage:1.0 → window.plugins.Llmusage
                          methods snapshot(), refresh(); signal updated(snapshot)
                          one shared poller per process (several widgets → one set of requests)
apps/edgectl              `edgectl llm-usage [--json]`, `edgectl llm-usage init` (writes example, 0600)
examples/widgets/llm-usage   HTML/JS/CSS widget (dev.edgehost.llm-usage) + node tests
examples/llm-usage/llm-usage.example.toml
docs/llm-usage.md         setup, sources, caveats
```

Config path: `$EDGEHOST_LLM_USAGE_CONFIG`, else `<EdgeHost data folder>/llm-usage.toml`. The
config is refused (error shown in the widget) if group/other can read it.

### Pace and "use next"

For a window with reset time `r`, length `L`, used fraction `u` at time `t`:
`elapsed = 1 − (r − t)/L`, `pace = u − elapsed` (positive: ahead, runs out early), projected
run-out `t + (1 − u)·(t − start)/u`. A subscription is **blocked** while any window is at 100%.
"Use next" ranks unblocked subscriptions by **urgency** = remaining fraction of the tightest window
÷ time fraction left in the window that resets soonest: big when lots of quota will expire soon.

## Steps (each a commit; `just ci` green before each)

1. This plan.
2. `crates/llm-usage`: model, pace, config, HTTP, parsers + fixture tests, credential stores
   (fake-store tests for refresh/write-back), service with cache + backoff.
3. edge-plugins built-in provider + shim/validate awareness; tests with a fake service.
4. `edgectl llm-usage` and `init`.
5. Widget (M-first layout, countdowns, pace marker, suggestion, error/stale states) + node tests.
6. Docs, example config, justfile (`js_test_dirs`, `llm-usage`, `llm-usage-init`, install recipe).
7. Live test against each service the owner has (Claude, Codex now; Gemini/Kimi/Ollama once the
   owner fills in the config), fix, run in a windowed slot M, review, commit.
