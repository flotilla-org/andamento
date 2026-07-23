#!/usr/bin/env python3
import argparse
import json
import os
import re
import subprocess
import sys
import time
import tempfile
import unittest
from pathlib import Path

OBSERVED_IDENTITIES_PIPE = "andamento-observed-identities"
METADATA_PATCH_PIPE = "andamento-apply-metadata-patch"
SOURCE_ID = "andamento-git-watcher"
SOURCE = "git-watcher"
DEFAULT_TTL_MS = 10_000
DEFAULT_FACTORY_LAYOUT = Path(__file__).resolve().parent.parent / "layouts" / "repo-manager-tab.kdl"
DEFAULT_FACTORY_NAME_PREFIX = "repo: "


def default_zellij_bin(environ=None, script_path=None):
    environ = environ or os.environ
    if environ.get("ZELLIJ_BIN"):
        return environ["ZELLIJ_BIN"]
    script_path = Path(script_path or __file__).resolve()
    sibling_dev_opt = script_path.parent.parent.parent / "zellij" / "target" / "dev-opt" / "zellij"
    if sibling_dev_opt.exists() and os.access(sibling_dev_opt, os.X_OK):
        return str(sibling_dev_opt)
    return "zellij"


def log(message):
    print(f"[andamento-git-watcher] {message}", flush=True)


def text_value(value):
    return {"type": "text", "value": value}


def path_text_value(value):
    return {"type": "text", "value": value}


def group_path_value(segments):
    return {"type": "group-path", "value": segments}


def group_path_segment(key, value, label=None):
    segment = {"key": key, "value": path_text_value(value)}
    if label:
        segment["label"] = label
    return segment


def metadata_value_update(value, ttl_ms=DEFAULT_TTL_MS):
    return {
        "value": value,
        "ttl_ms": ttl_ms,
        "precedence": None,
        "ordinal": None,
    }


def value_update(value, ttl_ms=DEFAULT_TTL_MS):
    return metadata_value_update(text_value(value), ttl_ms=ttl_ms)


def identity_target(key, value):
    return {
        "kind": "identity",
        "value": {
            "key": key,
            "value": text_value(value),
        },
    }


def tab_target(tab_id):
    return {"kind": "tab", "value": int(tab_id)}


def metadata_patch_for_target(target, facts, ttl_ms=DEFAULT_TTL_MS):
    facts = {"source": text_value(SOURCE), **facts}
    return {
        "type": "metadata-patch",
        "target": target,
        "source_id": SOURCE_ID,
        "set": {
            key: metadata_value_update(value, ttl_ms=ttl_ms)
            for key, value in sorted(facts.items())
            if value is not None
        },
        "unset": [],
    }


def metadata_patch(target_key, target_value, facts):
    return metadata_patch_for_target(
        identity_target(target_key, target_value),
        {key: text_value(value) for key, value in facts.items() if value},
    )


def observed_text_identities(observed_identities, key):
    values = []
    seen = set()
    for observed in observed_identities:
        identity = observed.get("identity", {})
        value = identity.get("value", {})
        if identity.get("key") == key and value.get("type") == "text":
            text = value.get("value", "")
            if text and text not in seen:
                seen.add(text)
                values.append(text)
    return values


def parse_repo(remote_url):
    patterns = [
        r"^git@github\.com:([^/]+)/(.+?)(?:\.git)?$",
        r"^https://github\.com/([^/]+)/(.+?)(?:\.git)?$",
        r"^ssh://git@github\.com/([^/]+)/(.+?)(?:\.git)?$",
    ]
    for pattern in patterns:
        match = re.match(pattern, remote_url)
        if match:
            return f"{match.group(1)}/{match.group(2)}"
    return None


def repo_name(repo):
    return repo.rsplit("/", 1)[-1] if repo else None


def run_text(args):
    completed = subprocess.run(args, check=True, capture_output=True, text=True)
    return completed.stdout.strip()


def git_facts(cwd):
    path = Path(cwd).expanduser()
    if not path.exists():
        return {}
    try:
        root = run_text(["git", "-C", str(path), "rev-parse", "--show-toplevel"])
    except (subprocess.CalledProcessError, FileNotFoundError):
        return {}
    facts = {"git.root": root}
    for key, args in [
        ("git.branch", ["git", "-C", root, "branch", "--show-current"]),
        ("git.remote.origin", ["git", "-C", root, "remote", "get-url", "origin"]),
    ]:
        try:
            facts[key] = run_text(args)
        except subprocess.CalledProcessError:
            pass
    repo = parse_repo(facts.get("git.remote.origin", ""))
    if repo:
        facts["vcs.repo"] = repo
        facts["repo.name"] = repo_name(repo)
    return facts


def get_observed_identities(verbose=False, zellij_bin="zellij", plugin_url=None):
    args = [zellij_bin, "pipe", "--name", OBSERVED_IDENTITIES_PIPE]
    if plugin_url:
        args += ["--plugin", plugin_url]
    output = run_text(args)
    if verbose:
        log(f"observed identity pipe returned {len(output)} bytes")
    return parse_observed_identities_output(output)


def parse_observed_identities_output(output):
    decoder = json.JSONDecoder()
    identities = []
    index = 0
    output = output or ""
    while index < len(output):
        while index < len(output) and output[index].isspace():
            index += 1
        if index >= len(output):
            break
        value, index = decoder.raw_decode(output, index)
        if isinstance(value, list):
            identities.extend(value)
    return identities


def publish_patch(patch, dry_run, zellij_bin="zellij", plugin_url=None):
    payload = json.dumps(patch, separators=(",", ":"))
    if dry_run:
        log(f"dry-run patch {payload}")
        return
    args = [zellij_bin, "pipe", "--name", METADATA_PATCH_PIPE]
    if plugin_url:
        args += ["--plugin", plugin_url]
    args += ["--", payload]
    subprocess.run(args, check=True)


def parse_tab_ids(output):
    tab_ids = []
    for line in (output or "").splitlines():
        text = line.strip()
        if text.isdigit():
            tab_ids.append(int(text))
    return tab_ids


def load_tabs(zellij_bin="zellij"):
    output = run_text([zellij_bin, "action", "list-tabs", "--json"])
    return json.loads(output or "[]")


def load_panes(zellij_bin="zellij"):
    output = run_text([zellij_bin, "action", "list-panes", "--all", "--json"])
    return json.loads(output or "[]")


def existing_repo_manager_tab_ids(repo, zellij_bin="zellij", name_prefix=DEFAULT_FACTORY_NAME_PREFIX):
    expected_name = f"{name_prefix}{repo}"
    try:
        tabs = load_tabs(zellij_bin=zellij_bin)
    except (subprocess.CalledProcessError, FileNotFoundError, json.JSONDecodeError) as error:
        log(f"could not list tabs for factory dedupe: {error}")
        return []
    return [
        int(tab["tab_id"])
        for tab in tabs
        if tab.get("name") == expected_name and "tab_id" in tab
    ]


def create_repo_manager_tab(repo, root, layout_path, zellij_bin="zellij", name_prefix=DEFAULT_FACTORY_NAME_PREFIX):
    output = run_text(
        [
            zellij_bin,
            "action",
            "new-tab",
            "--layout",
            str(layout_path),
            "--name",
            f"{name_prefix}{repo}",
            "--cwd",
            root,
        ]
    )
    return parse_tab_ids(output)


def repo_manager_tab_metadata(repo):
    name = repo_name(repo)
    return {
        "factory.id": text_value(f"repo-manager:{repo}"),
        "vcs.repo": text_value(repo),
        "repo.name": text_value(name),
        "tab.kind": text_value("repo-manager"),
        "tab.scope": group_path_value([
            group_path_segment("vcs.repo", repo, label=name),
        ]),
    }


def ensure_repo_manager_tab(
    facts,
    dry_run,
    verbose=False,
    zellij_bin="zellij",
    layout_path=DEFAULT_FACTORY_LAYOUT,
    created_repos=None,
):
    repo = facts.get("vcs.repo")
    root = facts.get("git.root")
    if not repo or not root:
        return []
    if created_repos is not None and repo in created_repos:
        return []

    existing_tab_ids = existing_repo_manager_tab_ids(repo, zellij_bin=zellij_bin)
    if existing_tab_ids:
        if verbose:
            log(f"repo-manager tab already exists for {repo}: {existing_tab_ids}")
        if created_repos is not None:
            created_repos.add(repo)
        return existing_tab_ids

    if verbose:
        log(f"creating repo-manager tab for {repo} at {root} using {layout_path}")
    if dry_run:
        log(f"dry-run create repo-manager tab for {repo} at {root}")
        return []

    tab_ids = create_repo_manager_tab(repo, root, layout_path, zellij_bin=zellij_bin)
    if not tab_ids:
        log(f"new-tab returned no tab ids for repo-manager {repo}")
        return []
    for tab_id in tab_ids:
        publish_patch(
            metadata_patch_for_target(
                tab_target(tab_id),
                repo_manager_tab_metadata(repo),
                ttl_ms=None,
            ),
            dry_run=False,
            zellij_bin=zellij_bin,
        )
    if created_repos is not None:
        created_repos.add(repo)
    return tab_ids


def current_tab_id(zellij_bin="zellij"):
    output = run_text([zellij_bin, "action", "current-tab-info", "--json"])
    info = json.loads(output)
    return int(info["tab_id"])


def parse_terminal_pane_id(value):
    if value is None:
        return None
    text = str(value).strip()
    if text.startswith("terminal_"):
        text = text.removeprefix("terminal_")
    if not text.isdigit():
        return None
    return int(text)


def tab_id_for_terminal_pane(panes, terminal_pane_id):
    for pane in panes:
        if pane.get("is_plugin"):
            continue
        if int(pane.get("id", -1)) == int(terminal_pane_id):
            return int(pane["tab_id"])
    return None


def scope_target_tab_id(zellij_bin="zellij", pane_id=None):
    terminal_pane_id = parse_terminal_pane_id(
        pane_id if pane_id is not None else os.environ.get("ZELLIJ_PANE_ID")
    )
    if terminal_pane_id is not None:
        try:
            tab_id = tab_id_for_terminal_pane(load_panes(zellij_bin=zellij_bin), terminal_pane_id)
            if tab_id is not None:
                return tab_id
        except (subprocess.CalledProcessError, FileNotFoundError, json.JSONDecodeError, KeyError, ValueError) as error:
            log(f"could not resolve helper pane tab from ZELLIJ_PANE_ID={terminal_pane_id}: {error}")
    return current_tab_id(zellij_bin=zellij_bin)


def scope_current_tab(scope_key, scope_value, scope_label, dry_run, zellij_bin="zellij", pane_id=None):
    tab_id = scope_target_tab_id(zellij_bin=zellij_bin, pane_id=pane_id)
    publish_patch(
        metadata_patch_for_target(
            tab_target(tab_id),
            {
                "tab.kind": text_value("andamento-control"),
                "tab.scope": group_path_value([
                    group_path_segment(scope_key, scope_value, label=scope_label),
                ]),
            },
            ttl_ms=None,
        ),
        dry_run=dry_run,
        zellij_bin=zellij_bin,
    )
    return tab_id


def run_once(
    dry_run,
    verbose=False,
    zellij_bin="zellij",
    factory_repo_manager=False,
    factory_layout=DEFAULT_FACTORY_LAYOUT,
    created_repos=None,
    plugin_url=None,
):
    observed = get_observed_identities(verbose=verbose, zellij_bin=zellij_bin, plugin_url=plugin_url)
    cwds = observed_text_identities(observed, "zellij.pane.cwd")
    if verbose:
        log(f"observed {len(observed)} identities; {len(cwds)} cwd identities")
    if not cwds and verbose:
        log("no zellij.pane.cwd identities to enrich")
    for cwd in cwds:
        if verbose:
            log(f"checking cwd {cwd}")
        facts = git_facts(cwd)
        if facts:
            if verbose:
                log(f"publishing {len(facts)} git facts for {cwd}: {', '.join(sorted(facts))}")
            publish_patch(metadata_patch("zellij.pane.cwd", cwd, facts), dry_run, zellij_bin=zellij_bin, plugin_url=plugin_url)
            if factory_repo_manager:
                ensure_repo_manager_tab(
                    facts,
                    dry_run,
                    verbose=verbose,
                    zellij_bin=zellij_bin,
                    layout_path=factory_layout,
                    created_repos=created_repos,
                )
        elif verbose:
            log(f"no git facts for {cwd}")


def main(argv):
    parser = argparse.ArgumentParser()
    parser.add_argument("--once", action="store_true")
    parser.add_argument("--dry-run", action="store_true")
    parser.add_argument("--interval", type=float, default=5.0)
    parser.add_argument("--test", action="store_true")
    parser.add_argument("--quiet", action="store_true")
    parser.add_argument("--zellij-bin", default=default_zellij_bin())
    parser.add_argument("--factory-repo-manager", action="store_true")
    parser.add_argument("--factory-layout", type=Path, default=DEFAULT_FACTORY_LAYOUT)
    parser.add_argument("--scope-current-tab", action="store_true")
    parser.add_argument("--scope-key", default="andamento.area")
    parser.add_argument("--scope-value", default="control")
    parser.add_argument("--scope-label", default="andamento")
    parser.add_argument("--scope-pane-id")
    parser.add_argument(
        "--plugin-url",
        default=None,
        help="Restrict pipe delivery to a specific plugin (passed to `zellij pipe --plugin`). "
             "Useful when multiple controllers are loaded; without this the pipe broadcasts.",
    )
    args = parser.parse_args(argv)
    if args.test:
        suite = unittest.defaultTestLoader.loadTestsFromTestCase(WatcherTests)
        result = unittest.TextTestRunner(verbosity=2).run(suite)
        return 0 if result.wasSuccessful() else 1
    if args.scope_current_tab:
        tab_id = scope_current_tab(
            args.scope_key,
            args.scope_value,
            args.scope_label,
            args.dry_run,
            zellij_bin=args.zellij_bin,
            pane_id=args.scope_pane_id,
        )
        if not args.quiet:
            log(f"scoped current tab {tab_id} to {args.scope_key}={args.scope_value}")
        return 0
    verbose = not args.quiet
    if verbose:
        mode = "dry-run" if args.dry_run else "publish"
        cadence = "once" if args.once else f"every {args.interval:g}s"
        log(f"starting ({mode}, {cadence})")
    created_repos = set()
    while True:
        run_once(
            args.dry_run,
            verbose=verbose,
            zellij_bin=args.zellij_bin,
            factory_repo_manager=args.factory_repo_manager,
            factory_layout=args.factory_layout,
            created_repos=created_repos,
            plugin_url=args.plugin_url,
        )
        if args.once:
            if verbose:
                log("done")
            return 0
        time.sleep(args.interval)


class WatcherTests(unittest.TestCase):
    def test_parse_repo(self):
        self.assertEqual(parse_repo("git@github.com:rjwittams/katzensteg.git"), "rjwittams/katzensteg")
        self.assertEqual(parse_repo("https://github.com/rjwittams/katzensteg"), "rjwittams/katzensteg")
        self.assertEqual(repo_name("rjwittams/katzensteg"), "katzensteg")

    def test_observed_text_identities_filters_by_key_and_type(self):
        observed = [
            {"identity": {"key": "zellij.pane.cwd", "value": text_value("/repo")}},
            {"identity": {"key": "zellij.pane.cwd", "value": text_value("/repo")}},
            {"identity": {"key": "vcs.repo", "value": text_value("rjwittams/katzensteg")}},
            {"identity": {"key": "zellij.pane.cwd", "value": {"type": "integer", "value": 1}}},
        ]
        self.assertEqual(observed_text_identities(observed, "zellij.pane.cwd"), ["/repo"])

    def test_parse_observed_identities_output_merges_multiple_pipe_responses(self):
        first = [{"identity": {"key": "zellij.pane.cwd", "value": text_value("/repo-a")}}]
        second = [{"identity": {"key": "zellij.pane.cwd", "value": text_value("/repo-b")}}]
        output = json.dumps(first) + "\n" + json.dumps(second)

        observed = parse_observed_identities_output(output)

        self.assertEqual(observed_text_identities(observed, "zellij.pane.cwd"), ["/repo-a", "/repo-b"])

    def test_metadata_patch_shape(self):
        patch = metadata_patch("zellij.pane.cwd", "/repo", {"vcs.repo": "rjwittams/katzensteg"})
        self.assertEqual(patch["type"], "metadata-patch")
        self.assertEqual(patch["source_id"], SOURCE_ID)
        self.assertEqual(patch["target"], identity_target("zellij.pane.cwd", "/repo"))
        self.assertEqual(patch["set"]["vcs.repo"]["value"], text_value("rjwittams/katzensteg"))
        self.assertEqual(patch["set"]["source"]["value"], text_value("git-watcher"))

    def test_parse_tab_ids_reads_plain_stdout_lines(self):
        self.assertEqual(parse_tab_ids("12\n13\n"), [12, 13])
        self.assertEqual(parse_tab_ids("created\n12\n"), [12])

    def test_repo_manager_tab_metadata_sets_typed_scope(self):
        metadata = repo_manager_tab_metadata("rjwittams/katzensteg")

        self.assertEqual(metadata["tab.kind"], text_value("repo-manager"))
        self.assertEqual(metadata["repo.name"], text_value("katzensteg"))
        self.assertEqual(
            metadata["tab.scope"],
            group_path_value([
                group_path_segment("vcs.repo", "rjwittams/katzensteg", label="katzensteg")
            ]),
        )

    def test_tab_metadata_patch_uses_tab_target_and_no_ttl(self):
        patch = metadata_patch_for_target(
            tab_target(7),
            repo_manager_tab_metadata("rjwittams/katzensteg"),
            ttl_ms=None,
        )

        self.assertEqual(patch["target"], tab_target(7))
        self.assertIsNone(patch["set"]["tab.scope"]["ttl_ms"])

    def test_tab_id_for_terminal_pane_reads_list_panes_json(self):
        panes = [
            {"id": 1, "is_plugin": True, "tab_id": 5},
            {"id": 2, "is_plugin": False, "tab_id": 7},
        ]

        self.assertEqual(tab_id_for_terminal_pane(panes, 2), 7)
        self.assertIsNone(tab_id_for_terminal_pane(panes, 1))

    def test_parse_terminal_pane_id_accepts_env_and_display_forms(self):
        self.assertEqual(parse_terminal_pane_id("3"), 3)
        self.assertEqual(parse_terminal_pane_id("terminal_4"), 4)
        self.assertIsNone(parse_terminal_pane_id("plugin_4"))

    def test_default_zellij_bin_prefers_env_then_sibling_fork_build(self):
        self.assertEqual(
            default_zellij_bin(environ={"ZELLIJ_BIN": "/tmp/custom-zellij"}),
            "/tmp/custom-zellij",
        )
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            script_path = root / "andamento" / "scripts" / "andamento-git-watcher.py"
            fork_bin = root / "zellij" / "target" / "dev-opt" / "zellij"
            script_path.parent.mkdir(parents=True)
            fork_bin.parent.mkdir(parents=True)
            fork_bin.write_text("#!/bin/sh\n")
            fork_bin.chmod(0o755)

            self.assertEqual(
                default_zellij_bin(environ={}, script_path=script_path),
                str(fork_bin.resolve()),
            )

    def test_default_zellij_bin_falls_back_to_path_lookup(self):
        with tempfile.TemporaryDirectory() as tmp:
            script_path = Path(tmp) / "andamento" / "scripts" / "andamento-git-watcher.py"
            script_path.parent.mkdir(parents=True)

            self.assertEqual(default_zellij_bin(environ={}, script_path=script_path), "zellij")


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
