# LLM Usage

The LLM Usage widget ([`examples/widgets/llm-usage`](../examples/widgets/llm-usage)) shows how
much quota each of your LLM subscriptions has left and when it resets. The aim is to use every
window's quota by its reset: to know when one runs out early and which other subscription still has
quota to spend.

Pieces:

| Part | Where |
|---|---|
| Fetchers, credentials, pace, "use next" | `crates/llm-usage` |
| Built-in plugin `edgehost.llmusage:LlmUsage:1.0` (`window.plugins.Llmusage`) | `crates/edge-plugins/src/llm_usage.rs` |
| `edgectl llm-usage` (check, `init`, `path`) | `apps/edgectl/src/llm_usage.rs` |
| Widget (HTML/JS/CSS) | `examples/widgets/llm-usage` |
| Example config | [`examples/llm-usage/llm-usage.example.toml`](../examples/llm-usage/llm-usage.example.toml) |

It is a built-in Rust provider, not a Rhai add-in, because it has to send `Authorization` and
`Cookie` headers and read the Keychain and the CLIs' credential files. Add-ins can do none of
these.

## Setup

```bash
edgectl llm-usage init     # writes llm-usage.toml (mode 600) into the EdgeHost data folder
$EDITOR "$(edgectl llm-usage path)"
edgectl llm-usage          # fetches every subscription and prints what the widget will show
```

The file is `$EDGEHOST_LLM_USAGE_CONFIG` if that is set. Otherwise it is `llm-usage.toml` in the
EdgeHost data folder: `edgehost --home`, else `$EDGEHOST_HOME`, else
`~/Library/Application Support/EdgeHost`. EdgeHost refuses the file if group or others can read it.
Unknown keys, a key that does not apply to the provider, and duplicate ids are all errors that name
the entry.

```toml
poll_seconds = 300            # at least 60

[[subscription]]
provider = "claude"           # Claude Code's login

[[subscription]]
provider = "codex"            # ~/.codex/auth.json

[[subscription]]
provider = "gemini"
cookie = "SID=…; __Secure-1PSID=…; …"

[[subscription]]
provider = "kimi"
api_key = "…"

[[subscription]]
provider = "ollama"
api_key = "…"
```

Any entry also takes `id`, `label` and `enabled = false`.

## Sources

All of these except Ollama's are private, undocumented endpoints, the same ones the vendors' own
CLIs and tools like CodexBar use. They can change without notice. When a parser no longer
understands a response, the widget says "unexpected response from …" instead of showing wrong
numbers. Researched 2026-10-16.

| Provider | Request | Windows | Credential |
|---|---|---|---|
| Claude Pro/Max | `GET api.anthropic.com/api/oauth/usage` + `anthropic-beta: oauth-2025-04-20` | `five_hour`, `seven_day` (% used) | Claude Code's OAuth login |
| Codex (ChatGPT plan) | `GET chatgpt.com/backend-api/wham/usage` + `ChatGPT-Account-Id` | primary (5 h), secondary (week) | Codex CLI's `auth.json` |
| Gemini app (AI Pro/Ultra) | `GET gemini.google.com/_/BardChatUi/graphql/…/executeQuery` (the usage page's query) | `GXU_FIVE_HOURLY`, `GXU_WEEKLY` | Browser `Cookie` header |
| Kimi Code | `GET api.kimi.com/coding/v1/usages` | 5 h, week, month | Kimi Code API key |
| Ollama Cloud | `GET ollama.com/api/balance` ([documented](https://docs.ollama.com/api/balance)) | monthly dollars (credit plans), or 5 h + week (legacy plans) | Ollama API key |

Model-specific weekly caps (Claude's `seven_day_opus`, Codex's `additional_rate_limits`) are not
shown, because they limit one model rather than the subscription.

### Claude

On macOS the token is read from the Keychain item `Claude Code-credentials` with
`/usr/bin/security`; elsewhere it comes from `~/.claude/.credentials.json`. macOS may ask you to
allow `security` the first time; choose **Always Allow**. The token needs the `user:profile` scope,
which a normal `claude` login has and a `claude setup-token` token does not.

### Codex

`$CODEX_HOME/auth.json` (default `~/.codex/auth.json`), with `auth_mode` `chatgpt`. An API-key login
has no subscription quota to show. Keyring-only Codex logins (`cli_auth_credentials_store =
"keyring"`) are not supported; point `auth_file` at a file, or give `access_token` and `account_id`.

### Gemini

Since 2026-06-18 Gemini CLI's "Login with Google" no longer serves consumer AI Pro/Ultra accounts,
and its quota API with it. The widget reads what [gemini.google.com/usage](https://gemini.google.com/usage)
shows instead. To get the cookie:

1. Open gemini.google.com/usage in a signed-in browser.
2. Open the developer tools (Safari: Develop → Show Web Inspector) and the Network tab, then reload.
3. Select the `usage` document request and copy the whole value of its `Cookie` request header.
4. Paste it as `cookie = "…"`.

The cookie is a full Google session: keep the file at mode 600. When Google signs it out, the
widget says so; paste a fresh one.

### Kimi and Ollama

API keys, from [kimi.com/code/console](https://kimi.com/code/console) and
[ollama.com/settings/keys](https://ollama.com/settings/keys). International Kimi accounts may need
`base_url = "https://api.kimi.ai/coding/v1"`. Ollama shows credits you bought on top of the
allowance as "+$N purchased"; they do not expire with the month.

## Token refresh and write-back

Claude Code and the Codex CLI rotate their refresh tokens: once a refresh token is used, the old one
stops working. So when EdgeHost refreshes an expired token, it must save the new one where the CLI
looks, or the CLI's login breaks. EdgeHost:

1. uses the stored access token while it is valid (with a 60 s margin), and refreshes only when it
   has expired or the service answers 401;
2. takes a process-wide lock, then **re-reads** the store. If the CLI or another widget refreshed
   it meanwhile, EdgeHost uses that token;
3. refreshes once (`platform.claude.com/v1/oauth/token`, `auth.openai.com/oauth/token`);
4. writes back the whole stored document with only the token fields changed:
   - the Keychain through `security -i`, so the secret never appears in `ps`;
   - `auth.json` atomically at mode 600.

A refused refresh (`invalid_grant`, `refresh_token_reused`) is reported as "sign in again" and
nothing is written. A race with the CLI refreshing at the same instant cannot be ruled out, because
there is no lock the CLI also honours. If it happens, one side gets "sign in again" and a fresh
`claude` / `codex login` fixes it. Tokens given in the config are never refreshed.

## Pace and "use next"

For a window of length `L` resetting at `r`, used fraction `u` at time `t`:

- `elapsed = 1 − (r − t)/L`. This is the white tick on the bar: where straight-line use would be.
- `ahead = u − elapsed`. Above +10% the window is amber: it runs out early, at
  `t + (1 − u)·elapsed·L/u`. Below −15%, once 20% of the window has passed, it is blue: about
  `1 − u/elapsed` of it will go unused.
- A subscription is **blocked** while any window is used up, until the last such window resets.
- **Urgency** is the worst use-it-or-lose-it ratio over the subscription's windows. For each window,
  take the quota it can really still use: its own remaining, capped by the remaining of every
  longer window, since spending it spends those too. Divide that by the share of its time left. For
  example, 62% of a week left with 13% of the week to go scores 4.8, and a fresh window scores 1.
  The widget's top line names the most urgent subscription that is not blocked.

## Polling and failures

One `llm_usage::Service` per process serves every LLM Usage widget, so two widgets do not double
the requests. It fetches all subscriptions in parallel at most once per `poll_seconds`. ↻ forces a
fetch, at most every 15 s. On HTTP 429 it waits out `Retry-After` (10 minutes if none is given)
before asking that service again. A failed fetch keeps the last good numbers, marked **stale**,
and treats a window whose reset time has passed as refilled.

## Testing

- `cargo test -p llm-usage`: parsers on recorded response shapes, pace and urgency golden values,
  config validation and the 0600 check, refresh and write-back against an in-memory store and a
  scripted HTTP fake, and the 401 → refresh → retry path against a temporary `auth.json`.
- `cargo test -p edge-plugins llm_usage`: the plugin's signals and thread lifetime.
- `node --test examples/widgets/llm-usage/`: the view model and the plugin wiring.
- Live: `edgectl llm-usage --config /tmp/x.toml` against real accounts.
