#!/usr/bin/env python3
import argparse
import json
import re
import subprocess
import sys
import time
import unittest
from pathlib import Path

OBSERVED_IDENTITIES_PIPE = "andamento-observed-identities"
METADATA_PATCH_PIPE = "tabs-apply-metadata-patch"
SOURCE_ID = "andamento-git-watcher"
DEFAULT_TTL_MS = 10_000


def text_value(value):
    return {"type": "text", "value": value}


def value_update(value, ttl_ms=DEFAULT_TTL_MS):
    return {
        "value": text_value(value),
        "ttl_ms": ttl_ms,
        "precedence": None,
        "ordinal": None,
    }


def identity_target(key, value):
    return {
        "kind": "identity",
        "value": {
            "key": key,
            "value": text_value(value),
        },
    }


def metadata_patch(target_key, target_value, facts):
    return {
        "MetadataPatch": {
            "target": identity_target(target_key, target_value),
            "source_id": SOURCE_ID,
            "set": {key: value_update(value) for key, value in sorted(facts.items()) if value},
            "unset": [],
        }
    }


def observed_text_identities(observed_identities, key):
    values = []
    for observed in observed_identities:
        identity = observed.get("identity", {})
        value = identity.get("value", {})
        if identity.get("key") == key and value.get("type") == "text":
            values.append(value.get("value", ""))
    return [value for value in values if value]


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
        facts["git.repo"] = repo
    return facts


def get_observed_identities():
    output = run_text(["zellij", "pipe", "--name", OBSERVED_IDENTITIES_PIPE])
    return json.loads(output or "[]")


def publish_patch(patch, dry_run):
    payload = json.dumps(patch, separators=(",", ":"))
    if dry_run:
        print(payload)
        return
    subprocess.run(
        ["zellij", "pipe", "--name", METADATA_PATCH_PIPE, "--", payload],
        check=True,
    )


def run_once(dry_run):
    observed = get_observed_identities()
    for cwd in observed_text_identities(observed, "zellij.pane.cwd"):
        facts = git_facts(cwd)
        if facts:
            publish_patch(metadata_patch("zellij.pane.cwd", cwd, facts), dry_run)


def main(argv):
    parser = argparse.ArgumentParser()
    parser.add_argument("--once", action="store_true")
    parser.add_argument("--dry-run", action="store_true")
    parser.add_argument("--interval", type=float, default=5.0)
    parser.add_argument("--test", action="store_true")
    args = parser.parse_args(argv)
    if args.test:
        suite = unittest.defaultTestLoader.loadTestsFromTestCase(WatcherTests)
        result = unittest.TextTestRunner(verbosity=2).run(suite)
        return 0 if result.wasSuccessful() else 1
    while True:
        run_once(args.dry_run)
        if args.once:
            return 0
        time.sleep(args.interval)


class WatcherTests(unittest.TestCase):
    def test_parse_repo(self):
        self.assertEqual(parse_repo("git@github.com:rjwittams/katzensteg.git"), "rjwittams/katzensteg")
        self.assertEqual(parse_repo("https://github.com/rjwittams/katzensteg"), "rjwittams/katzensteg")

    def test_observed_text_identities_filters_by_key_and_type(self):
        observed = [
            {"identity": {"key": "zellij.pane.cwd", "value": text_value("/repo")}},
            {"identity": {"key": "git.repo", "value": text_value("rjwittams/katzensteg")}},
            {"identity": {"key": "zellij.pane.cwd", "value": {"type": "integer", "value": 1}}},
        ]
        self.assertEqual(observed_text_identities(observed, "zellij.pane.cwd"), ["/repo"])

    def test_metadata_patch_shape(self):
        patch = metadata_patch("zellij.pane.cwd", "/repo", {"git.repo": "rjwittams/katzensteg"})
        body = patch["MetadataPatch"]
        self.assertEqual(body["source_id"], SOURCE_ID)
        self.assertEqual(body["target"], identity_target("zellij.pane.cwd", "/repo"))
        self.assertEqual(body["set"]["git.repo"]["value"], text_value("rjwittams/katzensteg"))


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
