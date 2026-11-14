#!/usr/bin/env python3
"""Bundle complete notices from the pinned native dependency graph."""
import argparse
import json
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
PIN = "70dbc4183f390a5a72d5dc01d8a77eb672217070"

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--checkouts", type=Path, required=True)
    parser.add_argument("--destination", type=Path, required=True)
    parser.add_argument("--target", default="aarch64-apple-darwin")
    parser.add_argument("--cargo-manifest", type=Path, default=ROOT / "Cargo.toml")
    args = parser.parse_args()
    lock = json.loads((ROOT / "apps/macos/macos/DitchAutomation/Package.resolved").read_text())
    packages = [("peekaboo", ROOT / "vendor/peekaboo", PIN, "https://github.com/openclaw/Peekaboo", True)]
    for pin in lock["pins"]:
        identity = pin["identity"]
        # Commander is resolved by Peekaboo's executable targets but not linked
        # into DitchAutomation. Preserve its notices in the inventory anyway.
        packages.append((identity, next(p for p in args.checkouts.iterdir() if p.name.lower() == identity), pin["state"]["revision"], pin["location"], identity != "commander"))
    notices, inventory = [], []
    for name, directory, revision, source, shipped in packages:
        files = sorted(p for p in directory.iterdir() if p.is_file() and
                       p.name.upper().startswith(("LICENSE", "NOTICE", "COPYING")))
        if not any(p.name.upper().startswith(("LICENSE", "COPYING")) for p in files):
            raise SystemExit(f"Missing license for {name}")
        inventory.append(dict(name=name, revision=revision, source=source, linked=shipped,
                              notices=[p.name for p in files]))
        for path in files:
            notices.append(f"\n{'=' * 72}\n{name} — {revision}\n{source}\n{path.name}\n\n{path.read_text()}")
    # Inventory the new MCP/async dependency closure as well as the native
    # engine. Include build-time dependencies conservatively; no inferred text
    # replaces a crate's actual copyright/license files.
    metadata = json.loads(subprocess.check_output([
        "cargo", "metadata", "--locked", "--format-version", "1",
        "--manifest-path", str(args.cargo_manifest), "--filter-platform", args.target,
    ], text=True))
    crates = {p["id"]: p for p in metadata["packages"]}
    nodes = {n["id"]: n for n in metadata["resolve"]["nodes"]}
    pending = [p["id"] for p in crates.values() if p["name"] in ("rmcp", "tokio")]
    visited = set()
    while pending:
        identifier = pending.pop()
        if identifier in visited:
            continue
        visited.add(identifier)
        pending.extend(d["pkg"] for d in nodes[identifier]["deps"]
                       if any(k["kind"] != "dev" for k in d["dep_kinds"]))
        crate = crates[identifier]
        directory = Path(crate["manifest_path"]).parent
        files = sorted(p for p in directory.iterdir() if p.is_file() and
                       p.name.upper().startswith(("LICENSE", "NOTICE", "COPYING")))
        if crate.get("license_file"):
            files = sorted(set(files + [directory / crate["license_file"]]))
        if not files:
            raise SystemExit(f"Missing crate license: {crate['name']}")
        inventory.append(dict(name=crate["name"], version=crate["version"],
                              source=crate["source"], license=crate["license"],
                              scope="MCP dependency graph", notices=[p.name for p in files]))
        for path in files:
            notices.append(f"\n{'=' * 72}\n{crate['name']} {crate['version']}\n{path.name}\n\n{path.read_text()}")
    args.destination.mkdir(parents=True, exist_ok=True)
    (args.destination / "OpenSourceLicenses.txt").write_text("Ditch Computer Use — native dependencies\n" + "\n".join(notices))
    (args.destination / "desktop-dependencies.json").write_text(json.dumps(inventory, indent=2) + "\n")

if __name__ == "__main__":
    main()
