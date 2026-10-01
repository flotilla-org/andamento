use super::*;
use andamento_core::{
    replay::{self, Frame, Replay},
    Sidebar,
};
use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};
static SERIAL: AtomicU64 = AtomicU64::new(0);
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "git-watcher-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn init(path: &Path) {
    fs::create_dir_all(path).unwrap();
    git(path, &["init", "-b", "main"]).unwrap();
    git(path, &["config", "user.email", "test@example.test"]).unwrap();
    git(path, &["config", "user.name", "Test"]).unwrap();
    git(path, &["commit", "--allow-empty", "-m", "initial"]).unwrap();
}
#[test]
fn discovers_linked_worktrees_and_refreshes_git_state() {
    let temp = Temp::new();
    let root = temp.0.join("scan/repo");
    init(&root);
    let linked = temp.0.join("outside quote' and space");
    git(
        &root,
        &["remote", "add", "origin", "git@example.test:team/repo.git"],
    )
    .unwrap();
    git(
        &root,
        &["worktree", "add", "-b", "feature", linked.to_str().unwrap()],
    )
    .unwrap();
    fs::create_dir(linked.join("subdir")).unwrap();
    let trees = discover(&[temp.0.join("scan")], &[linked.join("subdir")]);
    assert_eq!(trees.len(), 2);
    let tree = trees.iter().find(|t| t.root == linked).unwrap();
    assert!(tree.open);
    assert_eq!(tree.repo, "team/repo");
    assert_eq!(tree.branch, "feature");
    assert!(!tree.dirty);
    let root_tree = trees.iter().find(|t| t.root == root).unwrap();
    assert!(!root_tree.open);
    fs::write(linked.join("untracked"), "dirty").unwrap();
    git(&linked, &["checkout", "--detach"]).unwrap();
    let tree = discover(std::slice::from_ref(&linked), &[])
        .into_iter()
        .find(|t| t.root == linked)
        .unwrap();
    assert!(tree.branch.starts_with("detached:"));
    assert!(tree.dirty);
    assert!(!tree.open);
    let stream = patches(&[tree], 10_000);
    let recipe = stream[1]["set"]["action.primary.recipe"]["value"]["value"]
        .as_str()
        .unwrap();
    let cd = recipe.split(" && exec").next().unwrap();
    let output = Command::new("sh")
        .args(["-c", &format!("{cd} && pwd")])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        linked.to_str().unwrap()
    );
}
#[test]
fn remote_less_worktrees_share_parent_and_upstream_is_cleared() {
    let temp = Temp::new();
    let root = temp.0.join("repo");
    init(&root);
    let linked = temp.0.join("linked");
    git(
        &root,
        &["worktree", "add", "-b", "feature", linked.to_str().unwrap()],
    )
    .unwrap();
    git(&linked, &["branch", "--set-upstream-to=main"]).unwrap();
    let trees = discover(std::slice::from_ref(&root), &[]);
    assert_eq!(trees.len(), 2);
    assert_eq!(trees[0].repo, trees[1].repo);
    let tree = trees.iter().find(|t| t.root == linked).unwrap();
    assert_eq!(tree.upstream.as_deref(), Some("main"));
    assert_eq!(tree.ahead, Some(0));
    git(&linked, &["commit", "--allow-empty", "-m", "ahead"]).unwrap();
    assert_eq!(
        discover(std::slice::from_ref(&root), &[])
            .iter()
            .find(|t| t.root == linked)
            .unwrap()
            .ahead,
        Some(1)
    );
    git(&linked, &["branch", "--unset-upstream"]).unwrap();
    let stream = patches(&discover(&[root], &[]), 1000);
    assert!(stream
        .iter()
        .filter(|p| p["target"]["value"]["kind"] == "worktree")
        .all(|p| p["unset"]
            .as_array()
            .unwrap()
            .contains(&json!("git.upstream"))));
}
#[test]
fn parses_forge_remotes_and_host_inventories() {
    for remote in [
        "git@github.com:owner/repo.git",
        "https://forge.test/owner/repo.git",
        "ssh://git@forge.test/owner/repo",
    ] {
        assert_eq!(repo_name(remote).as_deref(), Some("owner/repo"));
    }
    assert_eq!(
        repo_name("https://forge.test/group/sub/repo.git").as_deref(),
        Some("group/sub/repo")
    );
    assert_eq!(repo_name("/local/repo"), None);
    let dirs = transport::workdirs(&json!({"workdirs":[{"cwd":"/old","live_cwd":"/new"},{"cwd":"/saved","live_cwd":null},{"cwd":"/saved","live_cwd":""},{"cwd":null}]})).unwrap();
    assert_eq!(dirs, vec![PathBuf::from("/new"), PathBuf::from("/saved")]);
    assert!(transport::workdirs(&json!({})).is_err());
    let response = r#"[{"identity":{"key":"zellij.pane.cwd","value":{"type":"text","value":"/repo"}}}] [{"identity":{"key":"zellij.pane.cwd","value":{"type":"text","value":"/repo"}}}]"#;
    assert_eq!(
        transport::observed_identities(response).unwrap(),
        vec![PathBuf::from("/repo")]
    );
}
#[test]
fn emitted_patches_replay_under_git_placement_and_expire() {
    let tree = Worktree {
        root: "/fixtures/repo".into(),
        repo: "owner/repo".into(),
        remote: "https://example.test/owner/repo.git".into(),
        branch: "main".into(),
        upstream: None,
        dirty: true,
        ahead: None,
        behind: None,
        open: false,
    };
    let stream = patches(&[tree], 10_000);
    let frames: Vec<Frame> = stream
        .into_iter()
        .map(|p| Frame {
            offset_ms: 0,
            patch: serde_json::from_value(p).unwrap(),
        })
        .collect();
    let mut replay = Replay::new(frames).unwrap();
    for config in [
        include_str!("../../../templates/andamento-git.kdl"),
        include_str!("../../../templates/flotilla-default.kdl"),
    ] {
        let mut sidebar = Sidebar::new(config).unwrap();
        // Use the same replay reader as checked-in connector captures.
        let capture = include_str!("../../../fixtures/git-watcher.jsonl");
        let mut recorded =
            Replay::new(replay::read(std::io::Cursor::new(capture)).unwrap()).unwrap();
        recorded.advance_to(&mut sidebar, 0).unwrap();
        let snapshot = sidebar.snapshot();
        let section = snapshot
            .surface
            .sections
            .iter()
            .find(|s| s.nodes.iter().any(|n| n.entity.kind == "repo"))
            .unwrap();
        for width in [24, 80] {
            let rendered = andamento_terminal::surface::render(&snapshot.surface, width);
            let lines = rendered.lines.join("\n");
            assert!(lines.contains("Git"), "{lines}");
            assert!(lines.contains("alpha"));
            assert!(lines.contains("feature"));
            if width == 80 {
                assert!(lines.contains("true"));
                assert!(lines.contains("open"));
            }
        }
        assert_eq!(section.nodes.len(), 2);
        assert_eq!(
            section
                .nodes
                .iter()
                .map(|n| n.children.len())
                .sum::<usize>(),
            4
        );
        assert!(section
            .nodes
            .iter()
            .all(|n| n.children.iter().all(|c| c.entity.kind == "worktree")));
        recorded.advance_to(&mut sidebar, 10_001).unwrap();
        assert!(sidebar
            .snapshot()
            .surface
            .sections
            .iter()
            .all(|s| s.nodes.is_empty()));
    }
    let mut sidebar = Sidebar::new(include_str!("../../../templates/andamento-git.kdl")).unwrap();
    replay.advance_to(&mut sidebar, 0).unwrap();
    assert_eq!(
        sidebar.snapshot().surface.sections[0].nodes[0]
            .children
            .len(),
        1
    );
}

#[cfg(unix)]
#[test]
fn wheelhouse_uses_unix_http_and_prefers_live_cwd() {
    use std::{
        io::{BufRead, BufReader, Read, Write},
        os::unix::net::UnixListener,
    };
    use transport::Transport;
    let temp = Temp::new();
    let socket = temp.0.join("ingress.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let server = std::thread::spawn(move || {
        for (index, expected) in [
            "GET /v1/observed/workdirs",
            "POST /v1/metadata/patch",
            "GET /v1/observed/workdirs",
        ]
        .into_iter()
        .enumerate()
        {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut first = String::new();
            reader.read_line(&mut first).unwrap();
            assert!(first.starts_with(expected));
            let mut length = 0;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" {
                    break;
                }
                if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = v.trim().parse().unwrap();
                }
            }
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            if expected.starts_with("POST") {
                let patch: Value = serde_json::from_slice(&body).unwrap();
                assert_eq!(patch["source_id"], SOURCE);
                stream.write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
            } else if index == 2 {
                stream.write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
            } else if first.contains("GET") && length == 0 {
                let body = r#"{"workdirs":[{"cwd":"/saved","live_cwd":"/live"}]}"#;
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                )
                .unwrap();
            }
        }
    });
    let mut host = transport::Wheelhouse { socket };
    assert_eq!(host.observed().unwrap(), vec![PathBuf::from("/live")]);
    host.publish(&entity_patch("repo", "test", BTreeMap::new(), vec![], 1000))
        .unwrap();
    assert!(host.observed().is_err());
    server.join().unwrap();
}

#[cfg(unix)]
#[test]
fn zellij_factory_dedupes_live_tabs_and_scopes_panes() {
    use std::os::unix::fs::PermissionsExt;
    use transport::Transport;
    let temp = Temp::new();
    let root = temp.0.join("repo");
    init(&root);
    git(
        &root,
        &["remote", "add", "origin", "git@example.test:owner/repo.git"],
    )
    .unwrap();
    let bin = temp.0.join("zellij");
    let log = temp.0.join("patches.jsonl");
    let state = temp.0.join("created");
    let observed =
        json!([{"identity":{"key":"zellij.pane.cwd","value":{"type":"text","value":root}}}]);
    let script = format!(
        r#"#!/bin/sh
set -eu
if [ "$1" = pipe ]; then
  [ "$4" = --plugin ] && [ "$5" = controller ]
  if [ "$3" = andamento-observed-identities ]; then
    printf '%s\n' {}
  else
    [ "$3" = andamento-apply-metadata-patch ] && [ "$6" = -- ]
    printf '%s\n' "$7" >> {}
  fi
elif [ "$2" = list-tabs ]; then
  if [ -f {} ]; then printf '%s\n' '[{{"name":"repo: owner/repo","tab_id":42}}]'; else printf '[]\n'; fi
elif [ "$2" = new-tab ]; then
  [ "$3" = --layout ] && [ "$4" = /factory.kdl ] && [ "$5" = --name ] && [ "$6" = 'repo: owner/repo' ] && [ "$7" = --cwd ] && [ "$8" = {} ]
  printf 'created\n' >> {}
  printf '42\n'
else exit 1
fi
"#,
        quote_shell(&observed.to_string()),
        quote_shell(log.to_str().unwrap()),
        quote_shell(state.to_str().unwrap()),
        quote_shell(root.to_str().unwrap()),
        quote_shell(state.to_str().unwrap())
    );
    fs::write(&bin, script).unwrap();
    fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
    let mut host = transport::Zellij {
        bin: bin.to_str().unwrap().into(),
        plugin: Some("controller".into()),
        factory_layout: Some("/factory.kdl".into()),
    };
    let observed = host.observed().unwrap();
    let trees = discover(&[], &observed);
    for _ in 0..2 {
        for patch in patches(&trees, 10_000) {
            host.publish(&patch).unwrap();
        }
        host.reconcile(&trees, &observed, 10_000).unwrap();
    }
    assert_eq!(fs::read_to_string(&state).unwrap().lines().count(), 1);
    let patches: Vec<Value> = fs::read_to_string(&log)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let pane = patches
        .iter()
        .find(|p| p["target"]["kind"] == "identity")
        .unwrap();
    assert_eq!(pane["set"]["entity.kind"]["value"]["value"], "worktree");
    let tab = patches
        .iter()
        .find(|p| p["target"]["kind"] == "tab")
        .unwrap();
    assert_eq!(tab["target"]["value"], 42);
    assert!(tab["set"]["entity.id"]["ttl_ms"].is_null());
    fs::remove_file(&state).unwrap();
    host.reconcile(&trees, &observed, 10_000).unwrap();
    assert!(state.exists());
}

#[test]
fn remotes_redact_credentials_and_handle_ports_and_file_urls() {
    assert_eq!(
        redact_remote("https://alice:pass@word@host/org/repo"),
        "https://host/org/repo"
    );
    let remote =
        redact_remote("https://alice:secret@forge.test:8443/org/repo.git?token=hidden#fragment");
    assert_eq!(remote, "https://forge.test:8443/org/repo.git");
    assert_eq!(repo_name(&remote).as_deref(), Some("org/repo"));
    assert_eq!(
        repo_name("ssh://git@host:22/org/repo").as_deref(),
        Some("org/repo")
    );
    assert_eq!(repo_name("file:///local/org/repo.git"), None);
    let temp = Temp::new();
    init(&temp.0);
    git(
        &temp.0,
        &[
            "remote",
            "add",
            "origin",
            "https://alice:secret@forge.test:8443/org/repo.git",
        ],
    )
    .unwrap();
    let stream = serde_json::to_string(&patches(
        &discover(std::slice::from_ref(&temp.0), &[]),
        1000,
    ))
    .unwrap();
    assert!(!stream.contains("secret"));
    assert!(!stream.contains("alice"));
    assert!(stream.contains("forge.test:8443/org/repo.git"));
}

#[test]
fn cycle_continues_after_publish_failure_and_retains_watch_roots() {
    use transport::Transport;
    struct Host {
        root: PathBuf,
        attempts: usize,
        reconciled: bool,
    }
    impl Transport for Host {
        fn observed(&mut self) -> io::Result<Vec<PathBuf>> {
            Ok(vec![self.root.clone()])
        }
        fn publish(&mut self, _: &Value) -> io::Result<()> {
            self.attempts += 1;
            if self.attempts == 1 {
                Err(io::Error::other("first rejected"))
            } else {
                Ok(())
            }
        }
        fn reconcile(&mut self, _: &[Worktree], _: &[PathBuf], _: u64) -> io::Result<()> {
            self.reconciled = true;
            Ok(())
        }
    }
    let temp = Temp::new();
    init(&temp.0);
    let mut host = Host {
        root: temp.0.clone(),
        attempts: 0,
        reconciled: false,
    };
    let cycle = refresh::run_cycle(&[], &mut host, 1000).unwrap();
    assert_eq!(cycle.trees.len(), 1);
    assert_eq!(cycle.errors, ["first rejected"]);
    assert_eq!(host.attempts, 2);
    assert!(host.reconciled);
    let watch = refresh::RefreshWatch::new(&cycle.trees);
    assert!(!watch.changed());
    git(&temp.0, &["checkout", "-b", "changed"]).unwrap();
    assert!(watch.changed());
    let watch = refresh::RefreshWatch::new(&cycle.trees);
    assert!(!watch.changed());
    fs::write(
        temp.0.join(".git/packed-refs"),
        "# pack-refs with: peeled fully-peeled sorted\n",
    )
    .unwrap();
    assert!(watch.changed());
}

#[test]
fn factory_layout_is_explicit_and_validated_before_host_actions() {
    assert!(refresh::factory_layout(true, None)
        .unwrap_err()
        .to_string()
        .contains("--factory-layout"));
    assert!(refresh::factory_layout(true, Some("/missing-layout.kdl".into())).is_err());
    assert!(refresh::factory_layout(false, None).unwrap().is_none());
    let temp = Temp::new();
    let layout = temp.0.join("tab.kdl");
    fs::write(&layout, "layout {}").unwrap();
    assert_eq!(
        refresh::factory_layout(true, Some(layout.clone())).unwrap(),
        Some(layout)
    );
}

#[cfg(unix)]
#[test]
fn discovery_ignores_symlink_cycles_and_unreadable_directories() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let temp = Temp::new();
    let root = temp.0.join("container");
    fs::create_dir(&root).unwrap();
    init(&root.join("good"));
    symlink(&root, root.join("loop")).unwrap();
    let blocked = root.join("blocked");
    init(&blocked);
    fs::set_permissions(&blocked, fs::Permissions::from_mode(0o0)).unwrap();
    let trees = discover(std::slice::from_ref(&root), &[]);
    fs::set_permissions(&blocked, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(trees.iter().any(|t| t.root == root.join("good")));
    if Command::new("id").arg("-u").output().unwrap().stdout != b"0\n" {
        assert_eq!(trees.len(), 1);
    }
}

#[cfg(unix)]
#[test]
fn non_utf8_paths_never_panic_or_create_lossy_recipes() {
    use std::os::unix::ffi::OsStringExt;
    use transport::Transport;
    let temp = Temp::new();
    init(&temp.0);
    let invalid = temp.0.join(std::ffi::OsString::from_vec(vec![0xff]));
    fs::create_dir(&invalid).unwrap();
    let trees = discover(std::slice::from_ref(&temp.0), &[]);
    let mut host = transport::Zellij {
        bin: "/not-invoked".into(),
        plugin: None,
        factory_layout: None,
    };
    host.reconcile(&trees, std::slice::from_ref(&invalid), 1000)
        .unwrap();
    let mut tree = trees[0].clone();
    tree.root = invalid;
    assert!(patches(&[tree], 1000).is_empty());
}

#[test]
fn labels_remain_nonempty_and_disambiguate_worktree_basenames() {
    assert_eq!(labels("-")["display.label.short"], "-");
    let temp = Temp::new();
    init(&temp.0);
    let mut trees = discover(std::slice::from_ref(&temp.0), &[]);
    trees[0].root = "/one/same".into();
    let mut second = trees[0].clone();
    second.root = "/two/same".into();
    second.branch = "other".into();
    trees.push(second);
    let stream = patches(&trees, 1000);
    assert_ne!(
        stream[1]["set"]["display.label"],
        stream[2]["set"]["display.label"]
    );
}

#[cfg(unix)]
#[test]
fn wheelhouse_times_out_when_server_does_not_respond() {
    use std::{
        os::unix::net::UnixListener,
        sync::mpsc,
        time::{Duration, Instant},
    };
    use transport::Transport;
    let temp = Temp::new();
    let socket = temp.0.join("hung.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let (done, wait) = mpsc::channel();
    let server = std::thread::spawn(move || {
        let (_stream, _) = listener.accept().unwrap();
        let _ = wait.recv_timeout(Duration::from_secs(12));
    });
    let start = Instant::now();
    let result = transport::Wheelhouse { socket }.observed();
    done.send(()).unwrap();
    server.join().unwrap();
    assert!(result.is_err());
    assert!(start.elapsed() >= Duration::from_secs(5));
    assert!(start.elapsed() < Duration::from_secs(12));
}

#[cfg(unix)]
#[test]
fn zellij_continues_after_pane_factory_and_tab_patch_failures() {
    use std::os::unix::fs::PermissionsExt;
    use transport::Transport;
    let temp = Temp::new();
    let roots: Vec<_> = ["a", "b", "c"]
        .into_iter()
        .map(|name| {
            let root = temp.0.join(name);
            init(&root);
            git(
                &root,
                &[
                    "remote",
                    "add",
                    "origin",
                    &format!("git@host:owner/{name}.git"),
                ],
            )
            .unwrap();
            root
        })
        .collect();
    let trees = discover(&roots, &roots);
    let log = temp.0.join("calls");
    let first = temp.0.join("first");
    let fail_inventory = temp.0.join("no-inventory");
    let bin = temp.0.join("zellij");
    fs::write(
        &bin,
        format!(
            r#"#!/bin/sh
set -eu
printf '%s\n' "$*" >> {}
if [ "$1" = pipe ]; then
  if [ ! -f {} ]; then touch {}; echo 'first pane failed' >&2; exit 1; fi
  case "$5" in *'"kind":"tab","value":42'*) echo 'tab failed' >&2; exit 1;; esac
elif [ "$2" = list-tabs ]; then
  if [ -f {} ]; then echo 'inventory unavailable' >&2; exit 1; fi
  printf '[]\n'
elif [ "$2" = new-tab ]; then
  case "$6" in
    'repo: owner/a') echo 'creation failed' >&2; exit 1;;
    'repo: owner/b') printf '42\n43\n';;
    'repo: owner/c') printf '44\n';;
  esac
fi
"#,
            quote_shell(log.to_str().unwrap()),
            quote_shell(first.to_str().unwrap()),
            quote_shell(first.to_str().unwrap()),
            quote_shell(fail_inventory.to_str().unwrap())
        ),
    )
    .unwrap();
    fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
    let mut host = transport::Zellij {
        bin: bin.to_str().unwrap().into(),
        plugin: None,
        factory_layout: Some("/factory.kdl".into()),
    };
    let error = host
        .reconcile(&trees, &roots, 1000)
        .unwrap_err()
        .to_string();
    for text in ["first pane failed", "creation failed", "tab failed"] {
        assert!(error.contains(text), "{error}");
    }
    let calls = fs::read_to_string(&log).unwrap();
    assert_eq!(
        calls
            .lines()
            .filter(|s| s.contains("\"kind\":\"identity\""))
            .count(),
        3
    );
    assert!(calls.contains("\"kind\":\"tab\",\"value\":43"));
    assert!(calls.contains("\"kind\":\"tab\",\"value\":44"));
    // A failed global inventory must not create blindly, but all panes refresh.
    fs::write(&log, "").unwrap();
    fs::write(&fail_inventory, "").unwrap();
    assert!(host.reconcile(&trees, &roots, 1000).is_err());
    let calls = fs::read_to_string(&log).unwrap();
    assert!(!calls.contains("new-tab"));
    assert_eq!(
        calls
            .lines()
            .filter(|s| s.contains("\"kind\":\"identity\""))
            .count(),
        3
    );
}

#[cfg(unix)]
#[test]
fn early_http_rejection_reports_curl_error_instead_of_broken_pipe() {
    use std::{io::Write, os::unix::net::UnixListener};
    use transport::Transport;
    let temp = Temp::new();
    let socket = temp.0.join("reject.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .write_all(
                b"HTTP/1.1 413 Content Too Large\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .unwrap();
    });
    let error = transport::Wheelhouse { socket }
        .publish(&json!({"large":"x".repeat(2_000_000)}))
        .unwrap_err()
        .to_string();
    server.join().unwrap();
    assert!(error.contains("Wheelhouse ingress:"), "{error}");
    assert!(error.contains("curl:"), "{error}");
}
