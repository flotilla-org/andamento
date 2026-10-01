//! Transport adapters. Only Zellij publishes identity/tab targets.
use crate::{patch, Worktree};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    io::{self, Write},
    path::PathBuf,
    process::{Command, Stdio},
};

pub trait Transport {
    fn observed(&mut self) -> io::Result<Vec<PathBuf>>;
    fn publish(&mut self, patch: &Value) -> io::Result<()>;
    fn reconcile(
        &mut self,
        _trees: &[Worktree],
        _observed: &[PathBuf],
        _ttl_ms: u64,
    ) -> io::Result<()> {
        Ok(())
    }
}

pub struct Stdout;
impl Transport for Stdout {
    fn observed(&mut self) -> io::Result<Vec<PathBuf>> {
        Ok(vec![])
    }
    fn publish(&mut self, patch: &Value) -> io::Result<()> {
        let mut out = io::stdout().lock();
        serde_json::to_writer(&mut out, patch)?;
        writeln!(out)?;
        out.flush()
    }
}

pub struct Wheelhouse {
    pub socket: PathBuf,
}
impl Wheelhouse {
    fn request(&self, route: &str, body: Option<&Value>) -> io::Result<String> {
        // curl handles HTTP framing (including chunked responses), socket timeouts
        // and status errors; no shell interpolation or network fallback is used.
        let mut command = Command::new("curl");
        command
            .args([
                "--silent",
                "--show-error",
                "--fail-with-body",
                "--max-time",
                "6",
                "--noproxy",
                "*",
                "--unix-socket",
            ])
            .arg(&self.socket)
            .arg(format!("http://localhost{route}"));
        if body.is_some() {
            command.args([
                "--header",
                "Content-Type: application/json",
                "--data-binary",
                "@-",
            ]);
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        if let Some(body) = body {
            serde_json::to_writer(child.stdin.take().unwrap(), body)?;
        }
        let output = child.wait_with_output()?;
        if !output.status.success() {
            return Err(io::Error::other(format!(
                "Wheelhouse ingress: {} {}",
                String::from_utf8_lossy(&output.stderr),
                String::from_utf8_lossy(&output.stdout)
            )));
        }
        String::from_utf8(output.stdout).map_err(io::Error::other)
    }
}

pub fn workdirs(value: &Value) -> io::Result<Vec<PathBuf>> {
    let entries = value
        .get("workdirs")
        .and_then(Value::as_array)
        .ok_or_else(|| io::Error::other("expected workdirs array"))?;
    let mut paths = Vec::new();
    for entry in entries {
        let path = entry
            .get("live_cwd")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .or_else(|| {
                entry
                    .get("cwd")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
            });
        if let Some(path) = path {
            paths.push(PathBuf::from(path));
        }
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}
impl Transport for Wheelhouse {
    fn observed(&mut self) -> io::Result<Vec<PathBuf>> {
        workdirs(&serde_json::from_str(
            &self.request("/v1/observed/workdirs", None)?,
        )?)
    }
    fn publish(&mut self, patch: &Value) -> io::Result<()> {
        self.request("/v1/metadata/patch", Some(patch)).map(|_| ())
    }
}

pub struct Zellij {
    pub bin: String,
    pub plugin: Option<String>,
    pub factory_layout: Option<PathBuf>,
}
impl Zellij {
    fn run(&self, args: &[&str]) -> io::Result<String> {
        let output = Command::new(&self.bin).args(args).output()?;
        if !output.status.success() {
            return Err(io::Error::other(
                String::from_utf8_lossy(&output.stderr).into_owned(),
            ));
        }
        String::from_utf8(output.stdout).map_err(io::Error::other)
    }
    fn pipe(&self, name: &str, payload: Option<&str>) -> io::Result<String> {
        let mut args = vec!["pipe", "--name", name];
        if let Some(plugin) = self.plugin.as_deref() {
            args.extend(["--plugin", plugin]);
        }
        if let Some(payload) = payload {
            args.extend(["--", payload]);
        }
        self.run(&args)
    }
}

pub fn observed_identities(output: &str) -> io::Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    // Broadcast pipes can concatenate multiple plugin responses.
    for value in serde_json::Deserializer::from_str(output).into_iter::<Value>() {
        let value = value?;
        let entries = value
            .as_array()
            .ok_or_else(|| io::Error::other("expected observed identities array"))?;
        for entry in entries {
            let identity = &entry["identity"];
            if identity["key"] == "zellij.pane.cwd" && identity["value"]["type"] == "text" {
                if let Some(path) = identity["value"]["value"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                {
                    paths.push(PathBuf::from(path));
                }
            }
        }
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

impl Transport for Zellij {
    fn observed(&mut self) -> io::Result<Vec<PathBuf>> {
        observed_identities(&self.pipe("andamento-observed-identities", None)?)
    }
    fn publish(&mut self, patch: &Value) -> io::Result<()> {
        self.pipe("andamento-apply-metadata-patch", Some(&patch.to_string()))
            .map(|_| ())
    }
    fn reconcile(
        &mut self,
        trees: &[Worktree],
        observed: &[PathBuf],
        ttl_ms: u64,
    ) -> io::Result<()> {
        for cwd in observed {
            // JSON identities cannot represent arbitrary Unix path bytes.
            let Some(cwd_text) = cwd.to_str() else {
                continue;
            };
            if let Some(root) = super::checkout(cwd) {
                if let Some(tree) = trees.iter().find(|t| t.root == root) {
                    self.publish(&patch(json!({"kind":"identity","value":{"key":"zellij.pane.cwd","value":{"type":"text","value":cwd_text}}}),
                        BTreeMap::from([("entity.kind".into(), json!("worktree")), ("entity.id".into(), json!(root.to_string_lossy())), ("git.root".into(), json!(root.to_string_lossy())), ("git.repo".into(), json!(tree.repo))]), vec![], ttl_ms))?;
                }
            }
        }
        if let Some(layout) = self.factory_layout.clone() {
            // Re-list on every refresh: a manually closed factory can be reopened,
            // and a failed creation is never recorded as successful.
            let tabs: Value =
                serde_json::from_str(&self.run(&["action", "list-tabs", "--json"])?)?;
            let tabs = tabs
                .as_array()
                .ok_or_else(|| io::Error::other("expected tab list"))?;
            let mut repos = std::collections::BTreeSet::new();
            for tree in trees {
                if !repos.insert(&tree.repo) {
                    continue;
                }
                let name = format!("repo: {}", tree.repo);
                let mut ids: Vec<u64> = tabs
                    .iter()
                    .filter(|t| t["name"] == name)
                    .filter_map(|t| t["tab_id"].as_u64())
                    .collect();
                if ids.is_empty() {
                    let output = self.run(&[
                        "action",
                        "new-tab",
                        "--layout",
                        &layout.to_string_lossy(),
                        "--name",
                        &name,
                        "--cwd",
                        &tree.root.to_string_lossy(),
                    ])?;
                    ids = output
                        .lines()
                        .filter_map(|line| line.trim().parse().ok())
                        .collect();
                    if ids.is_empty() {
                        return Err(io::Error::other("new-tab returned no tab ids"));
                    }
                }
                for id in ids {
                    let mut p = patch(
                        json!({"kind":"tab","value":id}),
                        BTreeMap::from([
                            ("entity.kind".into(), json!("repo")),
                            ("entity.id".into(), json!(tree.repo)),
                            ("git.repo".into(), json!(tree.repo)),
                            (
                                "action.primary.target".into(),
                                json!(format!("repo-manager:{}", tree.repo)),
                            ),
                        ]),
                        vec![],
                        ttl_ms,
                    );
                    for value in p["set"].as_object_mut().unwrap().values_mut() {
                        value["ttl_ms"] = Value::Null;
                    }
                    self.publish(&p)?;
                }
            }
        }
        Ok(())
    }
}
