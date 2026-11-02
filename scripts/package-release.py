#!/usr/bin/env python3
"""Package the native Rust executables and their source identity for a beta release."""

import argparse
import hashlib
import io
import json
from pathlib import Path
import subprocess
import tarfile
import tomllib
import zipfile


def package(binary_dir, output, target, source_version, build, commit):
    version = f"{source_version}-beta.{build}"
    name = f"knot-{version}-{target}"
    windows = "windows" in target
    suffix = ".exe" if windows else ""
    binaries = [binary_dir / f"{name}{suffix}" for name in ("knot", "knot-mcp")]
    for binary in binaries:
        if not binary.is_file() or binary.stat().st_size == 0:
            raise ValueError(f"Missing or empty release executable: {binary}")
    identity = json.dumps(
        dict(version=version, source_version=source_version, target=target, commit=commit),
        indent=2,
    ).encode() + b"\n"
    output.mkdir(parents=True, exist_ok=True)
    archive = output / (name + (".zip" if windows else ".tar.gz"))
    if windows:
        with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED) as bundle:
            for binary in binaries:
                bundle.write(binary, binary.name)
            bundle.writestr("BUILD-INFO.json", identity)
    else:
        with tarfile.open(archive, "w:gz") as bundle:
            for binary in binaries:
                info = bundle.gettarinfo(binary, arcname=binary.name)
                info.mode = 0o755
                with binary.open("rb") as contents:
                    bundle.addfile(info, contents)
            info = tarfile.TarInfo("BUILD-INFO.json")
            info.size = len(identity)
            info.mode = 0o644
            bundle.addfile(info, io.BytesIO(identity))
    with archive.open("rb") as contents:
        digest = hashlib.file_digest(contents, "sha256").hexdigest()
    archive.with_name(archive.name + ".sha256").write_text(
        f"{digest}  {archive.name}\n", encoding="utf-8", newline="\n"
    )
    return archive


def main():
    root = Path(__file__).resolve().parent.parent
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary-dir", type=Path, default=root / "target/release")
    parser.add_argument("--output", type=Path, default=root / "dist")
    parser.add_argument("--target")
    parser.add_argument("--build", type=int, required=True)
    parser.add_argument("--commit", required=True)
    args = parser.parse_args()
    if args.build < 1 or len(args.commit) != 40 or any(c not in "0123456789abcdef" for c in args.commit):
        parser.error("build must be positive and commit must be a full lowercase Git SHA")
    target = args.target
    if target is None:
        details = subprocess.check_output(["rustc", "-vV"], text=True)
        target = next(line.removeprefix("host: ") for line in details.splitlines() if line.startswith("host: "))
    if not target or any(c not in "abcdefghijklmnopqrstuvwxyz0123456789_-" for c in target):
        parser.error("invalid Rust target triple")
    with (root / "Cargo.toml").open("rb") as manifest:
        version = tomllib.load(manifest)["workspace"]["package"]["version"]
    print(package(args.binary_dir, args.output, target, version, args.build, args.commit))


if __name__ == "__main__":
    main()
