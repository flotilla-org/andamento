//! Host-independent repository discovery and expiring entity facts.
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::{Path, PathBuf},
    process::Command,
};

pub mod refresh;
pub mod transport;
pub const SOURCE: &str = "andamento-git-watcher";

#[derive(Debug, Clone)]
pub struct Worktree {
    pub root: PathBuf,
    pub repo: String,
    pub remote: String,
    pub branch: String,
    pub upstream: Option<String>,
    pub dirty: bool,
    pub ahead: Option<i64>,
    pub behind: Option<i64>,
    pub open: bool,
}

pub fn git(path: &Path, args: &[&str]) -> io::Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ));
    }
    String::from_utf8(output.stdout)
        .map(|s| s.trim_end_matches(['\n', '\r']).to_owned())
        .map_err(io::Error::other)
}

/// Never publish URL userinfo (which may contain a password or access token).
pub fn redact_remote(remote: &str) -> String {
    let Some((scheme, rest)) = remote.split_once("://") else {
        // Git's scp-style user@host:path has no password field; preserve it.
        return remote.to_owned();
    };
    let (authority, path) = rest
        .split_once('/')
        .map_or((rest, None), |(a, p)| (a, Some(p)));
    // A raw @ after the first slash can be a malformed, unencoded password
    // (or a legitimate path). Do not guess and risk publishing a secret.
    if path.is_some_and(|p| p.split(['?', '#']).next().unwrap_or(p).contains('@')) {
        return String::new();
    }
    let host = authority
        .rsplit('@')
        .next()
        .unwrap_or(authority)
        .split(['?', '#'])
        .next()
        .unwrap_or("");
    match path {
        Some(path) => format!(
            "{scheme}://{host}/{}",
            path.split(['?', '#']).next().unwrap_or(path)
        ),
        None => format!("{scheme}://{host}"),
    }
}

/// Preserve forge namespaces, while accepting HTTPS, SSH and scp-style remotes.
pub fn repo_name(remote: &str) -> Option<String> {
    let path = if let Some((scheme, rest)) = remote.split_once("://") {
        if !matches!(scheme, "http" | "https" | "ssh" | "git") {
            return None;
        }
        rest.split_once('/')?.1
    } else if remote.contains('@') {
        remote.split_once(':')?.1
    } else {
        return None;
    };
    let path = path.trim_matches('/').trim_end_matches(".git");
    path.contains('/').then(|| path.to_owned())
}

fn checkout(path: &Path) -> Option<PathBuf> {
    let root = git(path, &["rev-parse", "--show-toplevel"]).ok()?;
    fs::canonicalize(root).ok()
}

fn scan(path: &Path, visited: &mut BTreeSet<PathBuf>, found: &mut BTreeSet<PathBuf>) {
    let Ok(path) = fs::canonicalize(path) else {
        return;
    };
    if !visited.insert(path.clone()) {
        return;
    }
    if path.join(".git").exists() {
        if let Some(root) = checkout(&path) {
            found.insert(root);
        }
        return;
    }
    let Ok(entries) = fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_name() == ".git" {
            continue;
        }
        // Never follow directory symlinks outside a configured tree.
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            scan(&entry.path(), visited, found);
        }
    }
}

/// Configured roots are recursive containers or checkouts; observed paths can
/// be subdirectories. Worktree porcelain uses NUL delimiters for unusual paths.
pub fn discover(roots: &[PathBuf], observed: &[PathBuf]) -> Vec<Worktree> {
    let mut found = BTreeSet::new();
    let mut visited = BTreeSet::new();
    for root in roots {
        // One initial probe also accepts roots inside a checkout. Recursive
        // traversal only probes .git markers, not every ordinary directory.
        if let Some(checkout) = checkout(root) {
            found.insert(checkout);
        } else {
            scan(root, &mut visited, &mut found);
        }
    }
    let open: BTreeSet<_> = observed.iter().filter_map(|p| checkout(p)).collect();
    found.extend(open.iter().cloned());
    for root in found.clone() {
        if let Ok(output) = git(&root, &["worktree", "list", "--porcelain", "-z"]) {
            for field in output.split('\0') {
                if let Some(path) = field.strip_prefix("worktree ") {
                    if let Some(path) = checkout(Path::new(path)) {
                        found.insert(path);
                    }
                }
            }
        }
    }
    found
        .into_iter()
        .filter_map(|root| {
            let remote =
                redact_remote(&git(&root, &["remote", "get-url", "origin"]).unwrap_or_default());
            // The common directory gives remote-less linked worktrees one stable parent.
            let common = git(
                &root,
                &["rev-parse", "--path-format=absolute", "--git-common-dir"],
            )
            .ok()?;
            let repo = repo_name(&remote).unwrap_or_else(|| format!("local:{common}"));
            // One status process supplies branch, upstream, divergence and
            // dirty state. A failed status drops this tree rather than claiming
            // it is clean; its previously published facts expire by TTL.
            let status = git(&root, &["status", "--porcelain=v2", "--branch"]).ok()?;
            let (branch, upstream, ahead, behind, dirty) = parse_status(&status);
            Some(Worktree {
                open: open.contains(&root),
                root,
                repo,
                remote,
                branch,
                upstream,
                dirty,
                ahead,
                behind,
            })
        })
        .collect()
}

fn parse_status(status: &str) -> (String, Option<String>, Option<i64>, Option<i64>, bool) {
    let header = |name: &str| status.lines().find_map(|line| line.strip_prefix(name));
    let head = header("# branch.head ").unwrap_or("unborn");
    let branch = if head == "(detached)" {
        format!(
            "detached:{}",
            header("# branch.oid ")
                .unwrap_or("unknown")
                .chars()
                .take(12)
                .collect::<String>()
        )
    } else {
        head.to_owned()
    };
    let upstream = header("# branch.upstream ").map(str::to_owned);
    let mut counts = header("# branch.ab ").unwrap_or("").split_whitespace();
    let ahead = counts
        .next()
        .and_then(|s| s.strip_prefix('+'))
        .and_then(|s| s.parse().ok());
    let behind = counts
        .next()
        .and_then(|s| s.strip_prefix('-'))
        .and_then(|s| s.parse().ok());
    let dirty = status
        .lines()
        .any(|line| !line.is_empty() && !line.starts_with("# "));
    (branch, upstream, ahead, behind, dirty)
}

pub fn patch(
    target: Value,
    facts: BTreeMap<String, Value>,
    unset: Vec<&str>,
    ttl_ms: u64,
) -> Value {
    let set: BTreeMap<_, _> = facts.into_iter().map(|(key, value)| {
        let kind = if value.is_boolean() { "bool" } else if value.is_number() { "integer" } else { "text" };
        (key, json!({"value":{"type":kind,"value":value},"ttl_ms":ttl_ms,"precedence":null,"ordinal":null}))
    }).collect();
    json!({"type":"metadata-patch","target":target,"source_id":SOURCE,"set":set,"unset":unset})
}

pub fn entity_patch(
    kind: &str,
    id: &str,
    mut facts: BTreeMap<String, Value>,
    unset: Vec<&str>,
    ttl_ms: u64,
) -> Value {
    facts.insert("entity.kind".into(), json!(kind));
    facts.insert("entity.id".into(), json!(id));
    facts.insert("source".into(), json!("git-watcher"));
    patch(
        json!({"kind":"entity","value":{"kind":kind,"id":id}}),
        facts,
        unset,
        ttl_ms,
    )
}

pub fn quote_shell(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn labels(label: &str) -> BTreeMap<String, Value> {
    let medium = label.rsplit('/').next().unwrap_or(label);
    let mut short: String = medium
        .split(['-', '_', ' '])
        .filter_map(|s| s.chars().next())
        .collect();
    if short.is_empty() {
        short = medium.chars().take(1).collect();
    }
    BTreeMap::from([
        ("display.label".into(), json!(label)),
        ("display.label.medium".into(), json!(medium)),
        ("display.label.short".into(), json!(short)),
    ])
}

pub fn patches(worktrees: &[Worktree], ttl_ms: u64) -> Vec<Value> {
    let mut output = Vec::new();
    let mut repos = BTreeSet::new();
    for tree in worktrees {
        // Wire paths are UTF-8. Never invent a lossy recipe for another path.
        let Some(root) = tree.root.to_str() else {
            continue;
        };
        if repos.insert(&tree.repo) {
            let mut facts = labels(&tree.repo);
            facts.insert("git.repo".into(), json!(tree.repo));
            facts.insert("git.remote".into(), json!(tree.remote));
            output.push(entity_patch("repo", &tree.repo, facts, vec![], ttl_ms));
        }
        let basename = tree
            .root
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(root);
        let duplicate_name = worktrees
            .iter()
            .filter(|t| t.repo == tree.repo && t.root.file_name() == tree.root.file_name())
            .count()
            > 1;
        let label = if duplicate_name {
            format!("{basename} ({})", tree.branch)
        } else {
            basename.to_owned()
        };
        let mut facts = labels(&label);
        for (key, value) in [
            ("git.repo", json!(tree.repo)),
            ("git.root", json!(root)),
            ("git.branch", json!(tree.branch)),
            ("git.dirty", json!(tree.dirty)),
            ("git.open", json!(tree.open)),
            (
                "status.state",
                json!(if tree.open { "open" } else { "latent" }),
            ),
            ("action.primary.key", json!("open")),
            ("action.primary.label", json!("Open")),
            ("action.primary.target", json!(format!("worktree:{root}"))),
            (
                "action.primary.recipe",
                json!(format!(
                    "cd {} && exec \"${{SHELL:-/bin/sh}}\"",
                    quote_shell(root)
                )),
            ),
        ] {
            facts.insert(key.into(), value);
        }
        let mut unset = Vec::new();
        for (key, value) in [
            ("git.upstream", tree.upstream.as_ref().map(|s| json!(s))),
            ("git.ahead", tree.ahead.map(|n| json!(n))),
            ("git.behind", tree.behind.map(|n| json!(n))),
        ] {
            if let Some(value) = value {
                facts.insert(key.into(), value);
            } else {
                unset.push(key);
            }
        }
        output.push(entity_patch("worktree", root, facts, unset, ttl_ms));
    }
    output
}

#[cfg(test)]
mod tests;
