#!/usr/bin/env python3
"""Prepare the verified Ghostty source archive for the optional VT engine."""

import argparse
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tarfile
import tempfile
import urllib.request


def file_hash(file_path):
    with file_path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def tree_hash(root):
    """Hash relative names, file contents, and symlink targets without following links."""
    digest = hashlib.sha256()
    for current, directories, files in os.walk(root, followlinks=False):
        for name in sorted(directories + files):
            entry = Path(current) / name
            relative = entry.relative_to(root).as_posix()
            if entry.is_symlink():
                value = "link:" + os.readlink(entry)
            elif entry.is_file():
                value = "file:" + file_hash(entry)
            elif entry.is_dir():
                continue
            else:
                raise ValueError(f"unexpected source entry: {entry}")
            digest.update(relative.encode() + b"\0" + value.encode() + b"\0")
        directories.sort()
    return digest.hexdigest()


def verify_tree(root, expected):
    if root.is_symlink() or not root.is_dir() or tree_hash(root) != expected:
        raise ValueError(f"source contents differ from the pin: {root}; remove this generated directory and prepare again")


def download(item, directory):
    destination = directory / (item["name"] + ".tar.gz")
    if destination.exists():
        if file_hash(destination) != item["sha256"]:
            raise ValueError(f"archive checksum mismatch: {destination}")
        return destination
    request = urllib.request.Request(item["url"], headers={"User-Agent": "huterm-native-prepare"})
    with tempfile.NamedTemporaryFile(dir=directory, delete=False) as stream:
        temporary = Path(stream.name)
        try:
            with urllib.request.urlopen(request, timeout=120) as response:
                shutil.copyfileobj(response, stream)
            stream.flush()
            if file_hash(temporary) != item["sha256"]:
                raise ValueError(f"archive checksum mismatch for {item['name']}")
            temporary.replace(destination)
        finally:
            temporary.unlink(missing_ok=True)
    return destination


def prepare_source(item, root, archives, check):
    source = root / "source"
    if source.exists() or source.is_symlink():
        verify_tree(source, item["tree_sha256"])
        return
    if check:
        raise ValueError(f"missing native source: {source}")
    archive = download(item, archives)
    with tempfile.TemporaryDirectory(dir=root) as directory:
        stage = Path(directory)
        with tarfile.open(archive) as stream:
            stream.extractall(stage, filter="data")
        children = list(stage.iterdir())
        if len(children) != 1:
            raise ValueError("expected one root directory in the Ghostty archive")
        verify_tree(children[0], item["tree_sha256"])
        children[0].rename(source)


def verify_revision(manifest):
    repository = Path(__file__).resolve().parents[1]
    source = (repository / "crates/huterm-core/src/lib.rs").read_text()
    revision = re.search(r'pub const GHOSTTY_REVISION: &str =\s*"([0-9a-f]{40})";', source)
    provenance = json.loads((repository / "third-party/ghostty/provenance.json").read_text())
    if revision is None or revision.group(1) != manifest["revision"]:
        raise ValueError("Rust Ghostty revision differs from the native source manifest")
    expected_notice = f"https://github.com/ghostty-org/ghostty/blob/{manifest['revision']}/LICENSE"
    if provenance["ghostty-MIT.txt"]["url"] != expected_notice:
        raise ValueError("Ghostty license provenance differs from the native source manifest")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--directory", type=Path, default=Path(__file__).resolve().parents[1] / ".native/ghostty")
    parser.add_argument("--manifest", type=Path, default=Path(__file__).with_name("ghostty-source.json"))
    parser.add_argument("--check", action="store_true", help="verify existing sources without downloading or building")
    arguments = parser.parse_args()
    manifest = json.loads(arguments.manifest.read_text())
    verify_revision(manifest)
    root = arguments.directory.absolute()
    root.mkdir(parents=True, exist_ok=True)
    with (root / ".prepare.lock").open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        if not arguments.check:
            version = subprocess.check_output(["zig", "version"], text=True).strip()
            if version != manifest["zig"]:
                raise ValueError(f"expected Zig {manifest['zig']}, got {version}")
        archives = root / "archives"
        archives.mkdir(exist_ok=True)
        prepare_source(manifest["source"], root, archives, arguments.check)
    print(f"verified Ghostty {manifest['revision']} at {root / 'source'}")


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        raise SystemExit(f"Ghostty preparation failed: {error}") from error
