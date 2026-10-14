# EdgeHost data sources

Widgets run in a browser sandbox and cannot read the disk or run programs. A **data source** is a
read-only service EdgeHost provides on the widget's own origin, at `/__edgehost/<source>/<operation>`:

```js
const r = await fetch(`/__edgehost/git/status?repo=${encodeURIComponent(path)}`);
if (!r.ok) throw new Error(await r.text());   // 400 bad request, 403 not permitted, 404 not found, 502 failed
const status = await r.json();
```

These routes exist only under EdgeHost (not in iCUE on Windows); a widget should show a clear message
when they fail.

## Permissions

A data source only touches folders the widget declares in `manifest.json`:

```json
"permissions": [{ "type": "folder", "path": "<Home>/projects" }]
```

- Path macros: `<Home>`, `<Desktop>`, `<Documents>`, `<Downloads>`, `<Music>`, `<Pictures>`,
  `<Movies>`, `<Temp>`, `<AppData>` (EdgeHost's data folder), `<Runtime>` (macOS
  `~/Library/Application Support`, Linux `$XDG_DATA_HOME`). `~/` is also accepted.
- A request for a path outside every declared folder (after resolving symlinks) gets **403**.
- As with `url` permissions, declared folders are currently allowed without a prompt; user approval
  comes with milestone M3.

## `git`

Requires `git` on the host's `PATH`. Repositories are read with `--no-optional-locks`, and repository
hooks/fsmonitor are disabled for these calls. `repo` may be any folder inside a repository; `~/` is
expanded.

### `GET /__edgehost/git/status?repo=PATH`

```json
{
  "root": "/Users/me/projects/app",          // repository top level (canonical)
  "name": "app",                             // root folder name
  "branch": "main",                          // null when detached
  "detached": false,
  "head": { "hash": "9f2c…", "short": "9f2c1ab", "subject": "Fix tests", "author": "Me", "time": 1791946000 },
  "upstream": "origin/main",                 // null when none
  "ahead": 2, "behind": 0,                   // 0 when no upstream
  "staged": 1, "modified": 3, "untracked": 4, "conflicted": 0
}
```

`head` is null in a repository without commits. `time` values are Unix seconds.

### `GET /__edgehost/git/log?repo=PATH&n=20`

`n` is 1–100 (default 20). Commits on the current HEAD, newest first:

```json
{ "commits": [
  { "hash": "9f2c…", "short": "9f2c1ab", "author": "Me", "time": 1791946000,
    "subject": "Fix tests", "refs": ["HEAD -> main", "origin/main"] }
] }
```

### `GET /__edgehost/git/show?repo=PATH&rev=HASH`

`rev` must be 4–40 hex digits. One commit with its changed files:

```json
{ "hash": "9f2c…", "short": "9f2c1ab", "author": "Me", "time": 1791946000,
  "subject": "Fix tests", "body": "Longer description…",
  "files": [ { "path": "src/lib.rs", "additions": 12, "deletions": 3 },
             { "path": "logo.png", "additions": null, "deletions": null } ] }
```

`additions`/`deletions` are null for binary files.

### `GET /__edgehost/git/terminal`

The working directory of your most recent terminal prompt, if the shell hook below is installed:

```json
{ "cwd": "/Users/me/projects/app/src", "repo": "/Users/me/projects/app", "updated": 1791946100 }
```

`repo` is the repository top level containing `cwd`, or null. `cwd` is null when the hook is not
installed. A `cwd` outside the declared folders gets `repo: null` and `"error": "not permitted"`.

Shell hook (zsh; add to `~/.zshrc`):

```zsh
edgehost_cwd() {
  local dir="${XDG_STATE_HOME:-$HOME/.local/state}/edgehost"
  mkdir -p "$dir" && print -r -- "$PWD" >| "$dir/cwd"
}
autoload -Uz add-zsh-hook && add-zsh-hook chpwd edgehost_cwd && edgehost_cwd
```

(bash: `PROMPT_COMMAND='mkdir -p ~/.local/state/edgehost && pwd > ~/.local/state/edgehost/cwd'`.)
