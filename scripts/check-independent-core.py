#!/usr/bin/env python3
"""Build/test the reusable crates in a workspace containing no Zellij checkout.

Also links and runs a real C client against the exported ABI. The enclosing
plugin workspace still resolves sibling SDK paths even for `cargo -p core`;
this isolation check proves the actual library dependency graph separately.
"""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

root = Path(__file__).resolve().parents[1]
names = ["andamento-core", "andamento-terminal", "andamento-html", "andamento-ffi"]
host = next(line.split(": ", 1)[1] for line in subprocess.check_output(
    ["rustc", "-vV"], text=True).splitlines() if line.startswith("host: "))
target = root / "target" / "independent"

with tempfile.TemporaryDirectory(prefix="andamento-independent-") as directory:
    isolated = Path(directory)
    for name in names:
        shutil.copytree(root / "crates" / name, isolated / "crates" / name)
    shutil.copytree(root / "templates", isolated / "templates")
    shutil.copyfile(root / "Cargo.lock", isolated / "Cargo.lock")
    (isolated / "Cargo.toml").write_text('[workspace]\nresolver = "2"\nmembers = [' +
        ", ".join(json.dumps("crates/" + name) for name in names) + "]\n")
    env = dict(os.environ, CARGO_TARGET_DIR=str(target))
    # The smaller workspace prunes the copied lockfile; external versions stay pinned.
    metadata = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--format-version=1"], cwd=isolated, env=env, text=True))
    forbidden = [package["name"] for package in metadata["packages"]
                 if "zellij" in package["name"] or package["name"].startswith("flotilla")]
    assert not forbidden, forbidden
    subprocess.run(["cargo", "test", "--workspace", "--locked", "--target", host],
                   cwd=isolated, env=env, check=True)
    subprocess.run(["cargo", "check", "-p", "andamento-ffi", "--no-default-features",
                    "--locked", "--target", host], cwd=isolated, env=env, check=True)
    subprocess.run(["cargo", "build", "-p", "andamento-ffi", "--locked", "--target", host],
                   cwd=isolated, env=env, check=True)
    library = target / host / "debug"
    smoke = isolated / "ffi-smoke"
    subprocess.run(["cc", "-std=c11", "-Wall", "-Wextra", "-Werror", str(isolated / "crates/andamento-ffi/tests/smoke.c"),
                    "-I", str(isolated / "crates/andamento-ffi/include"),
                    "-L", str(library), "-landamento_ffi", "-o", str(smoke)], check=True)
    subprocess.run([str(smoke),
                    str(isolated / "crates/andamento-core/tests/fixtures/sidebar.kdl"),
                    str(isolated / "crates/andamento-core/tests/fixtures/sidebar.jsonl")], env=dict(env, LD_LIBRARY_PATH=str(library),
                                        DYLD_LIBRARY_PATH=str(library)), check=True)
    print("Independent core/frontends and C ABI verified without a Zellij or Flotilla dependency.")
