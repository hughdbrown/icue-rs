# LLM Usage

How much quota each of your LLM subscriptions has left, when it resets, and whether you are on pace
to use it all before then. The line at the top names the subscription to use next: the one with
the most quota about to expire unused.

![LLM Usage at Medium size in EdgeHost](screenshot.png)

*Medium (M) slot with live data. Claude Max has 61% of its week left with 14 hours to go, so the
widget suggests using Claude next. Its new 5-hour block has barely started. Codex Plus has not been
used yet.*

How each service is read, and its caveats, are in
[`docs/llm-usage.md`](../../../docs/llm-usage.md).

## Reading the widget

- **Bar**: share used. **White tick**: where straight-line use would be now, i.e. the "on pace"
  position.
- **Colours**:
  - green: on pace;
  - amber: ahead of pace, so it runs out before the reset at this rate;
  - accent (blue): behind pace, so quota will be left over;
  - red: used up;
  - grey: not started.
- **Right side**: amount left, then time to the reset and its local time.
- **Note line**: when it runs out, how much will go unused at the current pace, or when a used-up
  subscription comes back.
- **↻**: fetch now. Otherwise EdgeHost polls every `poll_seconds` (default 5 minutes), sharing one
  set of requests among all LLM Usage widgets.
- **Stale**: the last fetch failed, so the numbers are older; the reason is shown.
- **Error**: there are no numbers yet; the reason is shown.

## Requirements

- EdgeHost's built-in **LlmUsage** plugin (`edgehost.llmusage:LlmUsage:1.0`). It is not an iCUE
  plugin, so the widget does nothing in iCUE.
- A subscriptions file, `llm-usage.toml`, in the EdgeHost data folder
  (`~/Library/Application Support/EdgeHost/llm-usage.toml`), readable only by you. The widget
  itself has no credential settings.

## Configuration

### 1. Create the subscriptions file

```bash
edgectl llm-usage init     # writes a commented example with mode 600
open -e "$(edgectl llm-usage path)"
```

The file is a list of `[[subscription]]` entries. Every entry needs `provider`. It may also set:

- `label`: the display name;
- `id`: needed only when you have two entries for the same provider;
- `enabled = false`: keep the entry but skip it.

At the top, `poll_seconds` sets how often to fetch (default 300, at least 60).

Then check it:

```bash
edgectl llm-usage          # fetches every enabled subscription and prints what the widget will show
```

EdgeHost refuses the file if anyone else can read it. `init` sets the mode; if you created the file
yourself, run `chmod 600` on it.

### 2. Subscriptions

These are plans with a quota that resets on a schedule, which is what the widget is built for.

#### Anthropic: Claude Pro / Max

Shows the 5-hour block and the weekly limit as percentages.

```toml
[[subscription]]
provider = "claude"
label = "Claude Max"
```

- Uses Claude Code's login, so you need to be signed in: run `claude` and then `/login` with your
  Claude.ai account, not an API key.
- On macOS the login is in the Keychain item "Claude Code-credentials"; elsewhere it is
  `~/.claude/.credentials.json`.
- When the token expires, EdgeHost refreshes it and saves it back, as Claude Code would. The first
  time, macOS asks whether EdgeHost may change that Keychain item; choose **Always Allow**.
- A token from `claude setup-token` won't work: it lacks the `user:profile` scope that the usage
  endpoint needs.

#### OpenAI: ChatGPT Plus / Pro (Codex)

Shows Codex's 5-hour and weekly limits as percentages, plus any purchased credits.

```toml
[[subscription]]
provider = "codex"
label = "Codex"
# auth_file = "~/.codex/auth.json"   # the default ($CODEX_HOME/auth.json)
```

- Uses the Codex CLI's login: run `codex login` and choose **Sign in with ChatGPT**.
- An API-key login (`auth_mode = "apikey"` in `auth.json`) has no subscription quota to show.
- If you set `cli_auth_credentials_store = "keyring"` in `~/.codex/config.toml`, there is no
  `auth.json` to read. Switch back to `file`.
- Expired tokens are refreshed and written back to `auth.json`.

#### Google: Gemini app (Google AI Pro / Ultra)

Shows the Gemini app's 5-hour and weekly limits.

```toml
[[subscription]]
provider = "gemini"
label = "Gemini"
cookie = "SID=…; HSID=…; SSID=…; __Secure-1PSID=…; …"
```

Gemini CLI's Google login stopped reporting quota for consumer plans on 2026-06-18, so EdgeHost
reads what [gemini.google.com/usage](https://gemini.google.com/usage) shows. That needs your
browser's cookie:

1. Open gemini.google.com/usage in a browser where you are signed in.
2. Open the developer tools (Safari: **Develop → Show Web Inspector**; Chrome: **View → Developer
   → Developer Tools**), go to **Network**, and reload the page.
3. Select the first request (`usage`) and find **Request Headers → Cookie**.
4. Copy the whole value and paste it as `cookie = "…"`.

The cookie is a full Google session, so keep the file private. When Google signs it out, the widget
shows "the Gemini cookie is signed out"; paste a fresh one.

#### Kimi Code membership

Shows the 5-hour, weekly and monthly limits.

```toml
[[subscription]]
provider = "kimi"
label = "Kimi"
api_key = "sk-…"                                  # from kimi.com/code/console
# base_url = "https://api.kimi.ai/coding/v1"     # international accounts, if the default fails
```

#### Ollama Cloud (Free / Pro / Max)

Credit plans show dollars of the monthly allowance, plus any purchased credits. Legacy Pro/Max plans
show the 5-hour session and weekly limits.

```toml
[[subscription]]
provider = "ollama"
label = "Ollama Cloud"
api_key = "…"                                     # from ollama.com/settings/keys
```

This also covers cloud models you run through a local `ollama` (tags ending in `cloud`, such as
`gpt-oss:120b-cloud`): they count against the same Ollama Cloud plan.

#### Several accounts of one kind

Give each its own `id`:

```toml
[[subscription]]
provider = "codex"
id = "codex-work"
label = "Codex (work)"
auth_file = "~/work-codex/auth.json"

[[subscription]]
provider = "claude"
id = "claude-team"
label = "Claude (team)"
access_token = "sk-ant-oat01-…"   # a fixed OAuth token; never refreshed, so it stops when it expires
```

### 3. Pay-as-you-go APIs: not shown

The widget **cannot** track these, and you should leave them out of the file:

| Account | Why not |
|---|---|
| Anthropic API (console.anthropic.com credits) | There is no API that returns your remaining credit balance. The Admin API's [Usage and Cost API](https://platform.claude.com/docs/en/build-with-claude/usage-cost-api) reports spend (`/v1/organizations/cost_report`, daily buckets, admin key only), but not what is left. |
| OpenAI API (platform.openai.com credits) | The same: `GET /v1/organization/costs` reports spend with an admin key. OpenAI's developer forum [says there is no balance endpoint](https://community.openai.com/t/why-is-there-no-api-for-account-balance/577584) (checked 2026-10). |
| Gemini API key (Google AI Studio) | No quota or usage API at all. Limits show up only as HTTP 429 errors and on AI Studio's **Usage** dashboard. |

Pay-as-you-go credit also has no window to use up before a reset: unused credit simply carries
over, so there is nothing to be on pace for. Watch it in each console's billing page.

If you want spend shown against a monthly budget you choose, it can be built on the Anthropic and
OpenAI cost endpoints with an admin key, but it doesn't exist yet. The one pay-as-you-go balance
the widget already shows is Ollama Cloud's purchased credits.

### 4. Local models: nothing to configure

Models that run on your own machine (local `ollama` models, LM Studio, llama.cpp, MLX) have no
quota and no reset, so the widget has nothing to show for them, and there is no entry for them.
The exception is Ollama's **cloud** models run through the local `ollama` command (names ending in
`cloud`): they use your Ollama Cloud plan, so add the `ollama` entry above.

### 5. A complete example

```toml
poll_seconds = 300

[[subscription]]
provider = "claude"
label = "Claude Max"

[[subscription]]
provider = "codex"
label = "Codex"

[[subscription]]
provider = "gemini"
label = "Gemini"
cookie = "…"

[[subscription]]
provider = "kimi"
label = "Kimi"
api_key = "…"

[[subscription]]
provider = "ollama"
label = "Ollama Cloud"
api_key = "…"
```

### Troubleshooting

| The widget says | Do this |
|---|---|
| "Set up your subscriptions … does not exist" | `edgectl llm-usage init` |
| "… is readable by other users" | `chmod 600 "$(edgectl llm-usage path)"` |
| "…:4:1: unknown field `apikey`" | Fix the key name on that line (it is `api_key`) |
| "Claude Code is not signed in" / "sign in again" | Run `claude`, then `/login` |
| "Codex is not signed in with ChatGPT" | `codex login`, then **Sign in with ChatGPT** |
| "the Gemini cookie is signed out" | Paste a fresh Cookie header |
| "rate limited; retrying in N s" | Nothing; it waits as asked. Raise `poll_seconds` if it keeps happening |
| "unexpected response from …" | The service changed its private API; the widget needs an update |

## Settings

| Setting | Type | Default |
|---|---|---|
| Text Color, Accent Color, Background Color | color | `#E8ECF2`, `#7C9CFF`, `#14161B` |

## Run it

```bash
edgectl run-one examples/widgets/llm-usage --windowed --slot M   # try it in a stand-in window
edgectl install examples/widgets/llm-usage                       # or: just deploy-llm-usage
edgectl layout add dev.edgehost.llm-usage --slot M               # or Configure Widgets… in the menu
```

Designed for **M**. **S** shows one line per subscription with its first two windows. **L** and
**XL** put the cards in two columns.

## Files and tests

| Path | Contents |
|---|---|
| `index.html` | Settings and markup |
| `scripts/llm-model.js` | Pure view model: pace states, run-out and waste projections, the suggestion text |
| `scripts/llm-usage.js` | Plugin wiring and rendering |
| `styles/llm-usage.css` | Layout for S, M, L and XL |
| `llm-usage.test.mjs` | `node --test examples/widgets/llm-usage/` |

The Rust side (fetchers, credentials, pace and ranking) is `crates/llm-usage`, tested with
`cargo test -p llm-usage`.
