#!/usr/bin/env python3
"""Writes cargo-sources.json: the crates in Cargo.lock as flatpak-builder sources, vendored so
the build works offline. Run it whenever Cargo.lock changes:

    python3 packaging/flatpak/cargo-sources.py

Only crates.io dependencies are supported (that's all Familiar uses)."""
import json
import pathlib
import tomllib

ROOT = pathlib.Path(__file__).resolve().parents[2]
lock = tomllib.loads((ROOT / "Cargo.lock").read_text())
sources = []
for package in lock["package"]:
    source = package.get("source", "")
    if not source:
        continue  # a workspace crate
    if not source.startswith("registry+https://github.com/rust-lang/crates.io-index"):
        raise SystemExit(f"{package['name']}: only crates.io sources are supported ({source})")
    name, version, checksum = package["name"], package["version"], package["checksum"]
    dest = f"cargo/vendor/{name}-{version}"
    sources.append({
        "type": "archive",
        "archive-type": "tar-gzip",
        "url": f"https://static.crates.io/crates/{name}/{name}-{version}.crate",
        "sha256": checksum,
        "dest": dest,
    })
    sources.append({
        "type": "inline",
        "contents": json.dumps({"package": checksum, "files": {}}),
        "dest": dest,
        "dest-filename": ".cargo-checksum.json",
    })
sources.append({
    "type": "inline",
    "contents": '[source.vendored-sources]\ndirectory = "cargo/vendor"\n\n[source.crates-io]\nreplace-with = "vendored-sources"\n',
    "dest": "cargo",
    "dest-filename": "config.toml",
})
out = pathlib.Path(__file__).with_name("cargo-sources.json")
out.write_text(json.dumps(sources, indent=1) + "\n")
print(f"{out}: {len(sources) // 2} crates")
