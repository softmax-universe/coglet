"""Seal the builder's iOS framework. Native compiler checks remain in the builder."""
import argparse
import hashlib
import json
from pathlib import Path
import plistlib
import re
import stat
import zipfile


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def identity(tag, sha):
    if not re.fullmatch(r"ios-runtime-v[0-9]+\.[0-9]+\.[0-9]+(?:-rc[1-9][0-9]*)?", tag):
        raise ValueError("Invalid dedicated iOS runtime release tag")
    if not re.fullmatch(r"[0-9a-f]{40}", sha):
        raise ValueError("Expected full lowercase source SHA")


def inventory(framework):
    if framework.is_symlink() or framework.name != "CogletRuntime.xcframework":
        raise ValueError("Expected real CogletRuntime.xcframework directory")
    paths = sorted(framework.rglob("*"))
    if any(p.is_symlink() or not (p.is_file() or p.is_dir()) for p in paths):
        raise ValueError("Symlinks/special files are not distribution inputs")
    info = plistlib.loads((framework / "Info.plist").read_bytes())
    libs = info["AvailableLibraries"]
    expected = {("ios", None), ("ios", "simulator")}
    if len(libs) != 2 or {(x["SupportedPlatform"], x.get("SupportedPlatformVariant")) for x in libs} != expected:
        raise ValueError("Expected exactly iOS device and simulator slices")
    identifiers = set()
    for entry in libs:
        ident = entry["LibraryIdentifier"]
        if not re.fullmatch(r"[a-z0-9-]+", ident) or ident in identifiers:
            raise ValueError("Invalid or duplicate slice identifier")
        identifiers.add(ident)
        if entry["SupportedArchitectures"] != ["arm64"] or entry["LibraryPath"] != "libCogletRuntime.a" or entry["HeadersPath"] != "Headers":
            raise ValueError("Unexpected slice architecture/layout")
        directory = framework / ident
        archive = directory / "libCogletRuntime.a"
        with archive.open("rb") as stream:
            if stream.read(8) != b"!<arch>\n":
                raise ValueError("Expected static archive")
        for name in ("coglet_runtime.h", "module.modulemap"):
            if not (directory / "Headers" / name).is_file():
                raise ValueError("Missing C ABI/module header")
    return libs, [p for p in paths if p.is_file()]


def seal(framework, out, tag, sha, rust, xcode):
    identity(tag, sha)
    libs, paths = inventory(framework)
    out.mkdir(parents=True, exist_ok=False)
    archive = out / f"CogletRuntime-{tag}.xcframework.zip"
    with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED) as zipped:
        for path in paths:
            zipped.write(path, path.relative_to(framework.parent))
    manifest = {
        "format": 1, "tag": tag, "source_sha": sha,
        "engine": "wamr-interpreter", "minimum_ios": "15.0",
        "qualification": "device/simulator archive and SDK link; not simulator execution",
        "toolchain": {"rust": rust.strip(), "xcode": xcode.strip()},
        "slices": libs,
        "files": [{"name": p.relative_to(framework.parent).as_posix(), "size": p.stat().st_size, "sha256": digest(p)} for p in paths],
        "archive": {"name": archive.name, "size": archive.stat().st_size, "sha256": digest(archive)},
    }
    path = out / "ios-runtime.json"
    path.write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
    (out / "SHA256SUMS").write_text(f"{digest(archive)}  {archive.name}\n{digest(path)}  {path.name}\n")
    verify(out)
    return path


def verify(out):
    manifest = json.loads((out / "ios-runtime.json").read_text())
    identity(manifest["tag"], manifest["source_sha"])
    entry = manifest["archive"]
    if entry["name"] != f"CogletRuntime-{manifest['tag']}.xcframework.zip":
        raise ValueError("Unexpected archive name")
    archive = out / entry["name"]
    if archive.stat().st_size != entry["size"] or digest(archive) != entry["sha256"]:
        raise ValueError("Archive digest/size mismatch")
    files = manifest["files"]
    expected = {x["name"]: x for x in files}
    if len(expected) != len(files):
        raise ValueError("Duplicate file identity")
    with zipfile.ZipFile(archive) as zipped:
        names = zipped.namelist()
        if len(names) != len(expected) or set(names) != set(expected):
            raise ValueError("ZIP inventory mismatch")
        for item in zipped.infolist():
            name = item.filename
            if not name.startswith("CogletRuntime.xcframework/") or ".." in Path(name).parts or stat.S_ISLNK(item.external_attr >> 16):
                raise ValueError("Unsafe ZIP entry")
            data = zipped.read(item)
            if len(data) != expected[name]["size"] or hashlib.sha256(data).hexdigest() != expected[name]["sha256"]:
                raise ValueError("ZIP member digest/size mismatch")
    sums = f"{digest(archive)}  {archive.name}\n{digest(out / 'ios-runtime.json')}  ios-runtime.json\n"
    if (out / "SHA256SUMS").read_text() != sums:
        raise ValueError("Checksum inventory mismatch")
    return manifest


def main():
    parser = argparse.ArgumentParser(__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    build = sub.add_parser("seal")
    build.add_argument("--framework", type=Path, required=True)
    build.add_argument("--out", type=Path, required=True)
    for field in ("tag", "sha", "rust", "xcode"):
        build.add_argument(f"--{field}", required=True)
    check = sub.add_parser("verify")
    check.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    if args.command == "seal":
        seal(args.framework, args.out, args.tag, args.sha, args.rust, args.xcode)
    else:
        verify(args.out)


if __name__ == "__main__":
    main()
