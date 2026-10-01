//! Refresh orchestration and inexpensive Git control-file invalidation.
use crate::{discover, git, patches, transport::Transport, Worktree};
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
pub fn run_cycle(
    roots: &[PathBuf],
    transport: &mut dyn Transport,
    ttl_ms: u64,
) -> io::Result<Cycle> {
    let observed = transport.observed()?;
    let trees = discover(roots, &observed);
    let mut errors = Vec::new();
    for patch in patches(&trees, ttl_ms) {
        if let Err(error) = transport.publish(&patch) {
            errors.push(error.to_string());
        }
    }
    if let Err(error) = transport.reconcile(&trees, &observed, ttl_ms) {
        errors.push(error.to_string());
    }
    Ok(Cycle { trees, errors })
}

pub fn factory_layout(enabled: bool, layout: Option<PathBuf>) -> io::Result<Option<PathBuf>> {
    if !enabled {
        return Ok(None);
    }
    let layout = layout
        .ok_or_else(|| io::Error::other("--factory-repo-manager requires --factory-layout PATH"))?;
    if !layout.is_file() {
        return Err(io::Error::other(format!(
            "factory layout is not a file: {}",
            layout.display()
        )));
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
                    .filter_map(|name| {
                        git(
                            &t.root,
                            &["rev-parse", "--path-format=absolute", "--git-path", name],
                        )
                        .ok()
                        .map(PathBuf::from)
                    })
            })
            .collect::<Vec<_>>();
        let stamps = paths.iter().map(|p| stamp(p)).collect();
        Self { paths, stamps }
    }
    pub fn changed(&self) -> bool {
        self.paths
            .iter()
            .zip(&self.stamps)
            .any(|(path, before)| stamp(path) != *before)
    }
}
fn stamp(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).and_then(|m| m.modified()).ok()
}
