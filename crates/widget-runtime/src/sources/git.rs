//! The `git` data source: read-only repository status, history and commit details.
//!
//! `git` runs as a subprocess with an argument list (never a shell), with optional locks off and
//! the repository's fsmonitor and hooks disabled, so reading a repository cannot run its code.
//! Parsing is pure and tested separately from the subprocess.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::UNIX_EPOCH;

use serde_json::{Value, json};

use super::{Context, Query, SourceError};

/// Field and record separators used in `--format` strings.
const FS: char = '\u{1f}';
const RS: char = '\u{1e}';

/// Runs git; a seam for tests.
pub trait GitRunner {
    /// Run `git <args>` in `dir` and return stdout.
    ///
    /// # Errors
    ///
    /// [`SourceError::Failed`] with git's message, or when git is not installed.
    fn run(&self, dir: &Path, args: &[&str]) -> Result<String, SourceError>;
}

/// The `git` on `PATH`.
pub struct SystemGit;

impl GitRunner for SystemGit {
    fn run(&self, dir: &Path, args: &[&str]) -> Result<String, SourceError> {
        let output = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args([
                "--no-optional-locks",
                "-c",
                "core.fsmonitor=false",
                "-c",
                "core.hooksPath=/dev/null",
            ])
            .args(args)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("LC_ALL", "C")
            .output()
            .map_err(|e| SourceError::Failed(format!("cannot run git: {e}")))?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        } else {
            let msg = String::from_utf8_lossy(&output.stderr).trim().to_owned();
            Err(SourceError::Failed(if msg.is_empty() {
                format!("git exited with {}", output.status)
            } else {
                msg
            }))
        }
    }
}

/// Handle `/__edgehost/git/<op>`.
///
/// # Errors
///
/// Bad parameters, permission and git failures as [`SourceError`]s.
pub fn handle(
    ctx: &Context,
    git: &dyn GitRunner,
    op: &str,
    query: &Query,
) -> Result<Value, SourceError> {
    match op {
        "status" => status(git, &repo_root(ctx, git, query)?),
        "log" => {
            let n = match query.get("n") {
                None => 20,
                Some(s) => s
                    .parse::<u32>()
                    .ok()
                    .filter(|n| (1..=100).contains(n))
                    .ok_or_else(|| SourceError::BadRequest("n must be 1–100".into()))?,
            };
            log(git, &repo_root(ctx, git, query)?, n)
        }
        "show" => {
            let rev = query
                .get("rev")
                .ok_or_else(|| SourceError::BadRequest("rev is required".into()))?;
            if !(4..=40).contains(&rev.len()) || !rev.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(SourceError::BadRequest(
                    "rev must be 4–40 hex digits".into(),
                ));
            }
            show(git, &repo_root(ctx, git, query)?, rev)
        }
        "terminal" => Ok(terminal(ctx, git)),
        other => Err(SourceError::NotFound(format!(
            "git has no operation {other:?}"
        ))),
    }
}

/// The repository top level for the `repo` parameter, checked against the widget's folders.
fn repo_root(ctx: &Context, git: &dyn GitRunner, query: &Query) -> Result<PathBuf, SourceError> {
    let raw = query
        .get("repo")
        .ok_or_else(|| SourceError::BadRequest("repo is required".into()))?;
    let path = ctx
        .env
        .expand(raw)
        .ok_or_else(|| SourceError::BadRequest(format!("{raw} is not an absolute path")))?;
    let dir = ctx.access.check(&path)?;
    toplevel(git, &dir)
}

fn toplevel(git: &dyn GitRunner, dir: &Path) -> Result<PathBuf, SourceError> {
    match git.run(dir, &["rev-parse", "--show-toplevel"]) {
        Ok(out) => Ok(PathBuf::from(out.trim())),
        Err(SourceError::Failed(msg)) if msg.contains("not a git repository") => Err(
            SourceError::NotFound(format!("{} is not inside a git repository", dir.display())),
        ),
        Err(e) => Err(e),
    }
}

fn status(git: &dyn GitRunner, root: &Path) -> Result<Value, SourceError> {
    let porcelain = git.run(root, &["status", "--porcelain=v2", "--branch"])?;
    let mut s = parse_status(&porcelain);
    let head = git
        .run(
            root,
            &[
                "log",
                "-1",
                &format!("--format=%H{FS}%h{FS}%an{FS}%at{FS}%s"),
            ],
        )
        .ok()
        .and_then(|out| {
            parse_log(&format!("{}{RS}", out.trim_end_matches('\n')))
                .into_iter()
                .next()
        });
    let name = root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if let Value::Object(map) = &mut s {
        map.insert("root".into(), json!(root.display().to_string()));
        map.insert("name".into(), json!(name));
        map.insert("head".into(), head.unwrap_or(Value::Null));
    }
    Ok(s)
}

fn log(git: &dyn GitRunner, root: &Path, n: u32) -> Result<Value, SourceError> {
    let format = format!("--format=%H{FS}%h{FS}%an{FS}%at{FS}%s{FS}%D{RS}");
    match git.run(root, &["log", "-n", &n.to_string(), &format]) {
        Ok(out) => Ok(json!({ "commits": parse_log(&out) })),
        // A repository without commits has no history.
        Err(SourceError::Failed(msg)) if msg.contains("does not have any commits") => {
            Ok(json!({ "commits": [] }))
        }
        Err(e) => Err(e),
    }
}

fn show(git: &dyn GitRunner, root: &Path, rev: &str) -> Result<Value, SourceError> {
    let format = format!("--format=%H{FS}%h{FS}%an{FS}%at{FS}%s{FS}%b{RS}");
    let out = git
        .run(
            root,
            &[
                "show",
                "--numstat",
                "--no-ext-diff",
                "--no-color",
                &format,
                rev,
                "--",
            ],
        )
        .map_err(|e| match e {
            SourceError::Failed(msg)
                if msg.contains("unknown revision") || msg.contains("bad object") =>
            {
                SourceError::NotFound(format!("no commit {rev}"))
            }
            other => other,
        })?;
    parse_show(&out).ok_or_else(|| SourceError::Failed("unexpected git show output".into()))
}

fn terminal(ctx: &Context, git: &dyn GitRunner) -> Value {
    let state = std::env::var_os("XDG_STATE_HOME")
        .filter(|d| !d.is_empty())
        .map_or_else(|| ctx.env.home.join(".local/state"), PathBuf::from);
    let file = state.join("edgehost/cwd");
    let Ok(text) = std::fs::read_to_string(&file) else {
        return json!({ "cwd": null, "repo": null, "updated": null });
    };
    let updated = std::fs::metadata(&file)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs());
    let cwd = PathBuf::from(text.trim());
    let repo = match ctx.access.check(&cwd) {
        Ok(dir) => toplevel(git, &dir).ok().map(|p| p.display().to_string()),
        Err(SourceError::Forbidden(_)) => {
            return json!({
                "cwd": cwd.display().to_string(),
                "repo": null,
                "updated": updated,
                "error": "not permitted",
            });
        }
        Err(_) => None,
    };
    json!({ "cwd": cwd.display().to_string(), "repo": repo, "updated": updated })
}

/// Parse `git status --porcelain=v2 --branch`.
#[must_use]
pub fn parse_status(porcelain: &str) -> Value {
    let (mut branch, mut upstream, mut ahead, mut behind) =
        (None::<String>, None::<String>, 0_i64, 0_i64);
    let (mut staged, mut modified, mut untracked, mut conflicted) = (0_u32, 0_u32, 0_u32, 0_u32);
    for line in porcelain.lines() {
        if let Some(head) = line.strip_prefix("# branch.head ") {
            branch = (head != "(detached)").then(|| head.to_owned());
        } else if let Some(up) = line.strip_prefix("# branch.upstream ") {
            upstream = Some(up.to_owned());
        } else if let Some(ab) = line.strip_prefix("# branch.ab ") {
            for part in ab.split_whitespace() {
                if let Some(n) = part.strip_prefix('+') {
                    ahead = n.parse().unwrap_or(0);
                } else if let Some(n) = part.strip_prefix('-') {
                    behind = n.parse().unwrap_or(0);
                }
            }
        } else if line.starts_with("1 ") || line.starts_with("2 ") {
            let mut xy = line.get(2..4).unwrap_or("..").chars();
            if xy.next().is_some_and(|x| x != '.') {
                staged += 1;
            }
            if xy.next().is_some_and(|y| y != '.') {
                modified += 1;
            }
        } else if line.starts_with("u ") {
            conflicted += 1;
        } else if line.starts_with("? ") {
            untracked += 1;
        }
    }
    json!({
        "branch": branch,
        "detached": branch.is_none(),
        "upstream": upstream,
        "ahead": ahead,
        "behind": behind,
        "staged": staged,
        "modified": modified,
        "untracked": untracked,
        "conflicted": conflicted,
    })
}

/// Parse records of `%H FS %h FS %an FS %at FS %s [FS %D] RS`.
#[must_use]
pub fn parse_log(out: &str) -> Vec<Value> {
    out.split(RS)
        .map(|r| r.trim_start_matches('\n'))
        .filter(|r| !r.trim().is_empty())
        .filter_map(|record| {
            let f: Vec<&str> = record.split(FS).collect();
            let (hash, short, author, time, subject) =
                (f.first()?, f.get(1)?, f.get(2)?, f.get(3)?, f.get(4)?);
            let refs: Vec<&str> = f
                .get(5)
                .map(|d| {
                    d.split(", ")
                        .map(str::trim)
                        .filter(|r| !r.is_empty())
                        .collect()
                })
                .unwrap_or_default();
            Some(json!({
                "hash": hash,
                "short": short,
                "author": author,
                "time": time.trim().parse::<i64>().unwrap_or(0),
                "subject": subject,
                "refs": refs,
            }))
        })
        .collect()
}

/// Parse `git show --numstat` with a `%H FS %h FS %an FS %at FS %s FS %b RS` header.
#[must_use]
pub fn parse_show(out: &str) -> Option<Value> {
    let (header, stats) = out.split_once(RS)?;
    let f: Vec<&str> = header.split(FS).collect();
    let files: Vec<Value> = stats
        .lines()
        .filter_map(|line| {
            let mut parts = line.splitn(3, '\t');
            let (add, del, path) = (parts.next()?, parts.next()?, parts.next()?);
            let count = |s: &str| s.parse::<u64>().ok();
            Some(json!({ "path": path, "additions": count(add), "deletions": count(del) }))
        })
        .collect();
    Some(json!({
        "hash": f.first()?,
        "short": f.get(1)?,
        "author": f.get(2)?,
        "time": f.get(3)?.trim().parse::<i64>().unwrap_or(0),
        "subject": f.get(4)?,
        "body": f.get(5).map_or("", |b| b.trim_end()),
        "files": files,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::{Access, Env};

    #[test]
    fn status_parsing() {
        let out = "# branch.oid 9f2c\n# branch.head main\n# branch.upstream origin/main\n# branch.ab +2 -1\n\
                   1 M. N... 100644 100644 100644 a b src/a.rs\n1 .M N... 100644 100644 100644 a b src/b.rs\n\
                   1 MM N... 100644 100644 100644 a b c.rs\nu UU N... 1 2 3 4 a b c d x.rs\n? new.txt\n? other.txt\n";
        let s = parse_status(out);
        assert_eq!(s["branch"], "main");
        assert_eq!(
            (s["ahead"].as_i64(), s["behind"].as_i64()),
            (Some(2), Some(1))
        );
        assert_eq!(
            (s["staged"].as_u64(), s["modified"].as_u64()),
            (Some(2), Some(2))
        );
        assert_eq!(
            (s["untracked"].as_u64(), s["conflicted"].as_u64()),
            (Some(2), Some(1))
        );
        let detached = parse_status("# branch.oid abc\n# branch.head (detached)\n");
        assert_eq!(detached["detached"], true);
        assert_eq!(detached["upstream"], Value::Null);
    }

    #[test]
    fn log_and_show_parsing() {
        let out = format!(
            "aaaa{FS}aaa{FS}Ann{FS}100{FS}First{FS}HEAD -> main, origin/main{RS}\nbbbb{FS}bbb{FS}Bob{FS}90{FS}Second{FS}{RS}\n"
        );
        let log = parse_log(&out);
        assert_eq!(log.len(), 2);
        assert_eq!(log[0]["refs"], json!(["HEAD -> main", "origin/main"]));
        assert_eq!(log[1]["refs"], json!([]));
        let show = format!(
            "cccc{FS}ccc{FS}Cy{FS}80{FS}Third{FS}Body text\n{RS}\n3\t1\tsrc/x.rs\n-\t-\tlogo.png\n"
        );
        let s = parse_show(&show).unwrap();
        assert_eq!(s["body"], "Body text");
        assert_eq!(
            s["files"][0],
            json!({ "path": "src/x.rs", "additions": 3, "deletions": 1 })
        );
        assert_eq!(s["files"][1]["additions"], Value::Null);
    }

    /// End-to-end against the real `git` with a temporary repository (skipped without git).
    #[test]
    fn real_repository() {
        if Command::new("git").arg("--version").output().is_err() {
            return;
        }
        let home = tempfile::tempdir().unwrap();
        let repo = home.path().join("projects/demo");
        std::fs::create_dir_all(repo.join("src")).unwrap();
        let git = |args: &[&str]| {
            let ok = Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(args)
                .output()
                .unwrap()
                .status
                .success();
            assert!(ok, "git {args:?}");
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "user.email", "t@example.com"]);
        git(&["config", "user.name", "Tester"]);
        std::fs::write(repo.join("src/a.txt"), "one\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "First commit", "-m", "Details here"]);
        std::fs::write(repo.join("src/a.txt"), "one\ntwo\n").unwrap();
        std::fs::write(repo.join("new.txt"), "x").unwrap();

        let ctx = Context {
            access: Access::new(
                &["<Home>/projects".to_owned()],
                &Env {
                    home: home.path().to_path_buf(),
                    app_data: home.path().join("d"),
                },
            ),
            env: Env {
                home: home.path().to_path_buf(),
                app_data: home.path().join("d"),
            },
        };
        let q = |extra: &str| {
            Query::parse(Some(&format!("repo={}{extra}", repo.join("src").display())))
        };
        let s = handle(&ctx, &SystemGit, "status", &q("")).unwrap();
        assert_eq!(s["name"], "demo");
        assert_eq!(s["branch"], "main");
        assert_eq!(
            (s["modified"].as_u64(), s["untracked"].as_u64()),
            (Some(1), Some(1))
        );
        assert_eq!(s["head"]["subject"], "First commit");
        let log = handle(&ctx, &SystemGit, "log", &q("&n=5")).unwrap();
        let hash = log["commits"][0]["hash"].as_str().unwrap().to_owned();
        let show = handle(&ctx, &SystemGit, "show", &q(&format!("&rev={hash}"))).unwrap();
        assert_eq!(show["body"], "Details here");
        assert_eq!(
            show["files"][0],
            json!({ "path": "src/a.txt", "additions": 1, "deletions": 0 })
        );
        assert!(matches!(
            handle(&ctx, &SystemGit, "show", &q("&rev=--help")),
            Err(SourceError::BadRequest(_))
        ));
        assert!(matches!(
            handle(&ctx, &SystemGit, "log", &q("&n=0")),
            Err(SourceError::BadRequest(_))
        ));
        let outside = Query::parse(Some(&format!("repo={}", home.path().display())));
        assert!(matches!(
            handle(&ctx, &SystemGit, "status", &outside),
            Err(SourceError::Forbidden(_))
        ));
    }
}
