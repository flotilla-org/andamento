#!/usr/bin/env python3
"""Check the #105 project-row invariant at every scripted observation.

The checked-in gap/restart streams currently fail by design: their pinned
connector projection withdraws join facts. See fixtures/scenarios/README.md.
"""
import argparse
import json
from pathlib import Path
import subprocess


# Keep the placement invariant in sync with assert_standing_role_placement in
# crates/andamento-core/tests/replay.rs. Rust additionally checks replay exhaustion.
def main():
    root = Path(__file__).resolve().parents[1]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("capture", type=Path, help="replay JSONL fixture")
    parser.add_argument("--binary", type=Path, required=True, help="native andamento-replay executable")
    args = parser.parse_args()
    offsets = sorted({json.loads(line)["offset_ms"]
                      for line in args.capture.read_text().splitlines() if line.strip()})
    if not offsets:
        raise SystemExit("capture has no observations")
    key = None
    for at in offsets:
        try:
            raw = subprocess.check_output(
                [str(args.binary.resolve()), "snapshot", str(args.capture),
                 str(root / "fixtures/standing-role.kdl"), str(at)], stderr=subprocess.PIPE)
        except subprocess.CalledProcessError as error:
            detail = error.stderr.decode(errors="replace").strip()
            raise SystemExit(f"replay failed at {at} ms (exit {error.returncode}): {detail}") from None
        except OSError as error:
            raise SystemExit(f"could not run replay binary: {error}") from None
        projects = json.loads(raw)["surface"]["sections"][0]["nodes"]
        roles = [(project, role) for project in projects for role in project["children"]
                 if role["entity"]["kind"] == "role"]
        print(f"{at} ms: {len(roles)} project-row role actions", flush=True)
        if len(roles) != 1:
            raise SystemExit(f"role dissociated or duplicated at {at} ms")
        project, role = roles[0]
        if (project["entity"]["id"] != "flotilla/andamento@fleet"
                or role["entity"]["id"] != "flotilla/andamento/coder@fleet"):
            raise SystemExit(f"role moved to a different project or changed identity at {at} ms")
        if key is not None and role["key"] != key:
            raise SystemExit(f"role placement identity changed at {at} ms")
        key = role["key"]


if __name__ == "__main__":
    main()
