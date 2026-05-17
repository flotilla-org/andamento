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


def log(message):
    print(f"[andamento-git-watcher] {message}", flush=True)


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


def get_observed_identities(verbose=False):
    output = run_text(["zellij", "pipe", "--name", OBSERVED_IDENTITIES_PIPE])
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


def publish_patch(patch, dry_run):
    payload = json.dumps(patch, separators=(",", ":"))
    if dry_run:
        log(f"dry-run patch {payload}")
        return
    subprocess.run(
        ["zellij", "pipe", "--name", METADATA_PATCH_PIPE, "--", payload],
        check=True,
    )


def run_once(dry_run, verbose=False):
    observed = get_observed_identities(verbose=verbose)
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
            publish_patch(metadata_patch("zellij.pane.cwd", cwd, facts), dry_run)
        elif verbose:
            log(f"no git facts for {cwd}")


def main(argv):
    parser = argparse.ArgumentParser()
    parser.add_argument("--once", action="store_true")
    parser.add_argument("--dry-run", action="store_true")
    parser.add_argument("--interval", type=float, default=5.0)
    parser.add_argument("--test", action="store_true")
    parser.add_argument("--quiet", action="store_true")
    args = parser.parse_args(argv)
    if args.test:
        suite = unittest.defaultTestLoader.loadTestsFromTestCase(WatcherTests)
        result = unittest.TextTestRunner(verbosity=2).run(suite)
        return 0 if result.wasSuccessful() else 1
    verbose = not args.quiet
    if verbose:
        mode = "dry-run" if args.dry_run else "publish"
        cadence = "once" if args.once else f"every {args.interval:g}s"
        log(f"starting ({mode}, {cadence})")
    while True:
        run_once(args.dry_run, verbose=verbose)
        if args.once:
            if verbose:
                log("done")
            return 0
        time.sleep(args.interval)


class WatcherTests(unittest.TestCase):
    def test_parse_repo(self):
        self.assertEqual(parse_repo("git@github.com:rjwittams/katzensteg.git"), "rjwittams/katzensteg")
        self.assertEqual(parse_repo("https://github.com/rjwittams/katzensteg"), "rjwittams/katzensteg")

    def test_observed_text_identities_filters_by_key_and_type(self):
        observed = [
            {"identity": {"key": "zellij.pane.cwd", "value": text_value("/repo")}},
            {"identity": {"key": "zellij.pane.cwd", "value": text_value("/repo")}},
            {"identity": {"key": "git.repo", "value": text_value("rjwittams/katzensteg")}},
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
        patch = metadata_patch("zellij.pane.cwd", "/repo", {"git.repo": "rjwittams/katzensteg"})
        body = patch["MetadataPatch"]
        self.assertEqual(body["source_id"], SOURCE_ID)
        self.assertEqual(body["target"], identity_target("zellij.pane.cwd", "/repo"))
        self.assertEqual(body["set"]["git.repo"]["value"], text_value("rjwittams/katzensteg"))


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
