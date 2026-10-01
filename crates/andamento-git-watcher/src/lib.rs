//! Host-independent repository discovery and expiring entity facts.
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::{Path, PathBuf},
    process::Command,
};

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

/// Preserve forge namespaces, while accepting HTTPS, SSH and scp-style remotes.
pub fn repo_name(remote: &str) -> Option<String> {
    let path = if let Some((_, rest)) = remote.split_once("://") {
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
    if let Some(root) = checkout(&path) {
        found.insert(root);
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
        scan(root, &mut visited, &mut found);
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
            let remote = git(&root, &["remote", "get-url", "origin"]).unwrap_or_default();
            // The common directory gives remote-less linked worktrees one stable parent.
            let common = git(
                &root,
                &["rev-parse", "--path-format=absolute", "--git-common-dir"],
            )
            .ok()?;
            let repo = repo_name(&remote).unwrap_or_else(|| format!("local:{common}"));
            let branch = git(&root, &["symbolic-ref", "--quiet", "--short", "HEAD"])
                .unwrap_or_else(|_| {
                    git(&root, &["rev-parse", "--short", "HEAD"])
                        .map(|s| format!("detached:{s}"))
                        .unwrap_or_else(|_| "unborn".into())
                });
            let upstream = git(&root, &["rev-parse", "--abbrev-ref", "@{upstream}"]).ok();
            let counts = git(
                &root,
                &["rev-list", "--left-right", "--count", "HEAD...@{upstream}"],
            )
            .ok();
            let mut counts = counts
                .as_deref()
                .unwrap_or("")
                .split_whitespace()
                .map(str::parse::<i64>);
            let ahead = counts.next().and_then(Result::ok);
            let behind = counts.next().and_then(Result::ok);
            // Do not publish a false clean state when status fails.
            let dirty = !git(&root, &["status", "--porcelain"]).ok()?.is_empty();
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
    let short: String = medium
        .split(['-', '_', ' '])
        .filter_map(|s| s.chars().next())
        .collect();
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
        if repos.insert(&tree.repo) {
            let mut facts = labels(&tree.repo);
            facts.insert("git.repo".into(), json!(tree.repo));
            facts.insert("git.remote".into(), json!(tree.remote));
            output.push(entity_patch("repo", &tree.repo, facts, vec![], ttl_ms));
        }
        let root = tree.root.to_string_lossy();
        let mut facts = labels(
            tree.root
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or(&root),
        );
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
                    quote_shell(&root)
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
        output.push(entity_patch("worktree", &root, facts, unset, ttl_ms));
    }
    output
}

#[cfg(test)]
mod tests;
