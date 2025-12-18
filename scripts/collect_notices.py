#!/usr/bin/env python3
"""
Collect NOTICE-like files from the repository and produce a top-level NOTICE.

Cross-platform (Windows/Linux). If no NOTICE files are found, nothing is
written and the script prints a message and exits 0.

Usage:
  python scripts/collect_notices.py [--out NOTICE] [--overwrite]

Behavior mirrors the existing PowerShell helper but avoids creating an empty
NOTICE when none are found.
"""
from __future__ import annotations

import argparse
import hashlib
import os
import re
import shutil
import sys
from datetime import datetime
from pathlib import Path
import json
import subprocess
from typing import Optional


def find_notice_files(root: Path) -> list[Path]:
    pattern = re.compile(r"^NOTICE(\b|\.|$)", re.IGNORECASE)
    matches: list[Path] = []

    for dirpath, dirnames, filenames in os.walk(root):
        for fname in filenames:
            if pattern.match(fname):
                matches.append(Path(dirpath) / fname)

    return sorted(matches)


def cargo_metadata_repo_packages(repo_root: Path, verbose: bool = False) -> Optional[dict]:
    try:
        if verbose:
            print("Running: cargo metadata --format-version 1")
        proc = subprocess.run(
            ["cargo", "metadata", "--format-version", "1"],
            cwd=str(repo_root),
            check=True,
            capture_output=True,
            text=True,
        )
    except FileNotFoundError:
        if verbose:
            print("cargo not found in PATH; skipping external dependency scan")
        return None
    except subprocess.CalledProcessError as e:
        if verbose:
            print(f"cargo metadata failed: {e}; stderr: {e.stderr}")
        return None

    try:
        data = json.loads(proc.stdout)
        if verbose:
            names = [f"{p.get('name')}-{p.get('version')}" for p in data.get('packages', [])]
            print(f"cargo metadata returned {len(names)} packages (showing up to 40):")
            print(" ", ", ".join(names[:40]))
    except Exception as e:
        if verbose:
            print(f"failed to parse cargo metadata output: {e}; skipping")
        return None

    return data


def find_external_package_dirs(repo_root: Path, verbose: bool = False) -> list[Path]:
    """Return a list of external package source directories to scan for NOTICE files.

    This checks `cargo metadata`, and for each package whose manifest path is
    outside the repository, attempts to locate the package in the local cargo
    registry cache (~/.cargo/registry/src/*/<name>-<version>). Git/checkouts are
    ignored unless they appear under the registry layout.
    """
    data = cargo_metadata_repo_packages(repo_root, verbose=verbose)
    if not data:
        return []

    cargo_home = Path(os.environ.get("CARGO_HOME", Path.home() / ".cargo"))
    registry_src = cargo_home / "registry" / "src"

    out: list[Path] = []

    packages = data.get("packages", [])
    repo_root_str = str(repo_root.resolve())

    for pkg in packages:
        manifest = pkg.get("manifest_path")
        if not manifest:
            continue
        try:
            mpath = Path(manifest).resolve()
        except Exception:
            continue

        # Skip packages that are inside the workspace/repo
        try:
            if str(mpath).startswith(repo_root_str):
                continue
        except Exception:
            pass

        name = pkg.get("name")
        version = pkg.get("version")
        if not name or not version:
            continue

        # Look for the package directory in registry caches
        if registry_src.exists():
            for registry_dir in registry_src.iterdir():
                candidate = registry_dir / f"{name}-{version}"
                if candidate.exists():
                    out.append(candidate.resolve())
        # also consider the manifest's parent as a fallback
        parent = Path(manifest).parent
        if parent.exists() and parent not in out:
            out.append(parent.resolve())

    if verbose:
        print(f"Found {len(out)} candidate external package dirs before dedupe")

    # Deduplicate
    unique = []
    seen = set()
    for p in out:
        s = str(p)
        if s not in seen:
            seen.add(s)
            unique.append(p)

    return unique


def sha256_of(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(8192), b""):
            h.update(chunk)
    return h.hexdigest()


def make_backup(path: Path) -> Path:
    ts = datetime.now().strftime("%Y%m%d%H%M%S")
    bak = path.with_name(path.name + f".bak.{ts}")
    shutil.copy2(path, bak)
    return bak


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Aggregate NOTICE files into a top-level NOTICE")
    parser.add_argument("--out", "-o", default="NOTICE", help="Output NOTICE file name (default: NOTICE)")
    parser.add_argument("--overwrite", "-f", action="store_true", help="Overwrite existing NOTICE without backup")
    parser.add_argument("--verbose", "-v", action="store_true", help="Enable verbose debug output")
    args = parser.parse_args(argv)

    script_dir = Path(__file__).resolve().parent
    repo_root = script_dir.parent
    out_path = (repo_root / args.out).resolve()

    verbose = bool(args.verbose)
    print(f"Scanning for NOTICE files under: {repo_root}")

    candidates = find_notice_files(repo_root)

    # Also scan non-vendored dependencies discovered via cargo metadata
    try:
        external_dirs = find_external_package_dirs(repo_root, verbose=verbose)
    except Exception as e:
        print(f"Warning: error locating external package dirs: {e}")
        external_dirs = []

    for d in external_dirs:
        candidates += find_notice_files(d)

    # Exclude target NOTICE file itself
    candidates = [p.resolve() for p in candidates if p.resolve() != out_path]

    if not candidates:
        print("No NOTICE files found. Not creating a NOTICE file.")
        return 0

    seen: dict[str, bool] = {}
    sections: list[str] = []

    for p in candidates:
        try:
            key = sha256_of(p)
        except Exception as e:
            print(f"Warning: could not hash {p}: {e}")
            key = None

        if key and key in seen:
            print(f"Skipping duplicate: {p}")
            continue

        if key:
            seen[key] = True

        rel = os.path.relpath(p, repo_root)
        sections.append(f"---- Source: {rel} ----")
        try:
            text = p.read_text(encoding="utf-8")
        except Exception:
            text = p.read_text(encoding="latin-1")
        sections.append(text)
        sections.append("\n")

    header = [
        "Aggregated NOTICE file",
        f"Generated: {datetime.now().isoformat()}",
        "Included files:",
        f"{len(seen)} files",
        "",
    ]

    content = "\n".join(header + sections)

    if out_path.exists() and not args.overwrite:
        bak = make_backup(out_path)
        print(f"Existing {out_path.name} backed up to: {bak}")

    out_path.write_text(content, encoding="utf-8")
    print(f"Wrote aggregated NOTICE to: {out_path}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
