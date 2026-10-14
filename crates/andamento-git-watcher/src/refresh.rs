//! Refresh orchestration and inexpensive Git control-file invalidation.
use crate::{discover, git, patches, transport::Transport, withdrawals, Worktree};
use serde_json::Value;
use std::{
    fs, io,
    path::{Path, PathBuf},
    time::SystemTime,
};

pub struct Cycle {
    pub trees: Vec<Worktree>,
    pub errors: Vec<String>,
}

/// Failed discovery is not an empty inventory. Individual publish failures do
/// not suppress later entities or discard the invalidation watch list.
/// `published` is the last cycle's entity patches: what it had and this one
/// does not is withdrawn. `ttl_ms` is the lease on panes' facts.
pub fn run_cycle(roots: &[PathBuf], transport: &mut dyn Transport, published: &mut Vec<Value>, ttl_ms: u64) -> io::Result<Cycle> {
    let observed = transport.observed()?;
    let trees = discover(roots, &observed);
    let mut errors = Vec::new();
    let current = patches(&trees);
    for patch in withdrawals(published, &current).iter().chain(&current) {
        if let Err(error) = transport.publish(patch) {
            errors.push(error.to_string());
        }
    }
    *published = current;
    if let Err(error) = transport.reconcile(&trees, &observed, ttl_ms) {
        errors.push(error.to_string());
    }
    Ok(Cycle { trees, errors })
}

pub fn factory_layout(enabled: bool, layout: Option<PathBuf>) -> io::Result<Option<PathBuf>> {
    if !enabled {
        return Ok(None);
    }
    let layout = layout.ok_or_else(|| io::Error::other("--factory-repo-manager requires --factory-layout PATH"))?;
    if !layout.is_file() {
        return Err(io::Error::other(format!("factory layout is not a file: {}", layout.display())));
    }
    Ok(Some(layout))
}

pub struct RefreshWatch {
    paths: Vec<PathBuf>,
    stamps: Vec<Option<SystemTime>>,
}
impl RefreshWatch {
    pub fn new(trees: &[Worktree]) -> Self {
        let paths = trees
            .iter()
            .flat_map(|t| {
                ["HEAD", "index", "packed-refs"]
                    .into_iter()
                    .filter_map(|name| git(&t.root, &["rev-parse", "--path-format=absolute", "--git-path", name]).ok().map(PathBuf::from))
            })
            .collect::<Vec<_>>();
        let stamps = paths.iter().map(|p| stamp(p)).collect();
        Self { paths, stamps }
    }
    pub fn changed(&self) -> bool {
        self.paths.iter().zip(&self.stamps).any(|(path, before)| stamp(path) != *before)
    }
}
fn stamp(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).and_then(|m| m.modified()).ok()
}
