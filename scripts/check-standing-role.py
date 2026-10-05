#!/usr/bin/env python3
"""Check the #105 project-row invariant at every scripted observation.

The checked-in gap/restart streams currently fail by design: their pinned
connector projection withdraws join facts. See fixtures/scenarios/README.md.
"""
import argparse
import json
from pathlib import Path
import subprocess


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
        raw = subprocess.check_output([str(args.binary.resolve()), "snapshot",
                                       str(args.capture), str(root / "fixtures/standing-role.kdl"), str(at)])
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
