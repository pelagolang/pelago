#!/usr/bin/env python3
"""Release helper. The one place that knows the version rules.

Standard library only. Subcommands:

  nightly-version --date YYYYMMDD --sha SHA [--tags FILE]   next nightly version, from the stable tags
  tag-info TAG                                              channel, prerelease and latest flags of a tag
  check-tag TAG                                             the tag's base version equals compiler/Cargo.toml
  package --binary B --target T --version V --out DIR       one archive (tar.gz, or zip on Windows)
  sums DIR                                                  write DIR/SHA256SUMS over every archive
  notes --to REF [--from TAG]                               changelog from Conventional Commits
  prune-nightlies --names FILE --keep N                     asset names to delete, oldest versions first
"""
import argparse
import hashlib
import os
import re
import shutil
import subprocess
import sys
import tarfile
import zipfile

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
STABLE = re.compile(r"^v(\d+)\.(\d+)\.(\d+)$")
TAG = re.compile(r"^v(\d+)\.(\d+)\.(\d+)(?:-(beta|rc)\.(\d+))?$")


def cargo_version():
    with open(os.path.join(ROOT, "compiler", "Cargo.toml"), encoding="utf-8") as f:
        m = re.search(r'^version\s*=\s*"([^"]+)"', f.read(), re.M)
    if not m:
        sys.exit("no version in compiler/Cargo.toml")
    return m.group(1)


def nightly_version(date, sha, tags):
    """0.1.0 before any stable release; afterwards the next minor of the newest stable tag."""
    stable = [tuple(map(int, m.groups())) for t in tags if (m := STABLE.match(t))]
    if stable:
        major, minor, _ = max(stable)
        base = f"{major}.{minor + 1}.0"
    else:
        base = "0.1.0"
    return f"{base}-nightly.{date}+{sha[:7]}"


def tag_info(tag):
    m = TAG.match(tag)
    if not m:
        sys.exit(f"{tag}: not a release tag (vX.Y.Z, vX.Y.0-beta.N or vX.Y.0-rc.N)")
    major, minor, patch, pre, n = m.groups()
    if pre and patch != "0":
        sys.exit(f"{tag}: beta and rc releases are cut for X.Y.0 only")
    channel = pre or "stable"
    return {
        "version": tag[1:],
        "base": f"{major}.{minor}.{patch}",
        "channel": channel,
        "prerelease": "true" if pre else "false",
        "latest": "false" if pre else "true",
    }


def archive_name(version, target):
    ext = "zip" if "windows" in target else "tar.gz"
    return f"pelago-{version}-{target}.{ext}"


def package(binary, target, version, out):
    os.makedirs(out, exist_ok=True)
    exe = "pelago.exe" if "windows" in target else "pelago"
    path = os.path.join(out, archive_name(version, target))
    top = f"pelago-{version}-{target}"
    files = [(binary, f"{top}/{exe}")]
    for extra in ("LICENSE-MIT", "LICENSE-APACHE", "README.md"):
        files.append((os.path.join(ROOT, extra), f"{top}/{extra}"))
    if path.endswith(".zip"):
        with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as z:
            for src, arc in files:
                z.write(src, arc)
    else:
        with tarfile.open(path, "w:gz") as t:
            for src, arc in files:
                info = t.gettarinfo(src, arc)
                info.uid = info.gid = 0
                info.uname = info.gname = ""
                if arc.endswith("/pelago"):
                    info.mode = 0o755
                with open(src, "rb") as f:
                    t.addfile(info, f)
    return path


def sha256(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def write_sums(directory):
    names = sorted(n for n in os.listdir(directory) if n.endswith((".tar.gz", ".zip")))
    if not names:
        sys.exit(f"{directory}: no archives")
    with open(os.path.join(directory, "SHA256SUMS"), "w", encoding="utf-8", newline="\n") as f:
        for n in names:
            f.write(f"{sha256(os.path.join(directory, n))}  {n}\n")
    return names


SECTIONS = [
    ("feat", "Features"),
    ("fix", "Fixes"),
    ("perf", "Performance"),
    ("refactor", "Refactoring"),
    ("docs", "Documentation"),
    ("test", "Tests"),
    ("build", "Build"),
    ("ci", "CI"),
    ("chore", "Chores"),
]
CC = re.compile(r"^(\w+)(?:\(([^)]+)\))?(!)?:\s*(.+)$")


def notes(to_ref, from_tag):
    rng = f"{from_tag}..{to_ref}" if from_tag else to_ref
    out = subprocess.run(
        ["git", "log", "--no-merges", "--format=%h%x1f%s%x1f%b%x1e", rng],
        cwd=ROOT, capture_output=True, text=True, check=True,
    ).stdout
    breaking, groups = [], {}
    for rec in out.split("\x1e"):
        rec = rec.strip("\n")
        if not rec:
            continue
        h, subject, body = (rec.split("\x1f") + ["", ""])[:3]
        m = CC.match(subject)
        if not m:
            groups.setdefault("other", []).append(f"- {subject} ({h})")
            continue
        typ, scope, bang, text = m.groups()
        line = f"- {'**' + scope + ':** ' if scope else ''}{text} ({h})"
        if bang or "BREAKING CHANGE:" in body:
            breaking.append(line)
        groups.setdefault(typ, []).append(line)
    parts = []
    if breaking:
        parts.append("### Breaking changes\n\n" + "\n".join(breaking))
    for typ, title in SECTIONS + [("other", "Other")]:
        if typ in groups:
            parts.append(f"### {title}\n\n" + "\n".join(groups[typ]))
    return "\n\n".join(parts) if parts else "No changes recorded."


def prune(names, keep):
    """Nightly asset names look like pelago-<version>-<target>.<ext>; keep the newest N versions."""
    rx = re.compile(r"^pelago-(.+-nightly\.(\d{8})\+[0-9a-f]+)-")
    versions = {}
    for n in names:
        m = rx.match(n)
        if m:
            versions[m.group(1)] = m.group(2)
    ordered = sorted(versions, key=lambda v: (versions[v], v), reverse=True)
    drop = set(ordered[keep:])
    sums = [n for n in names if re.match(r"^SHA256SUMS", n)]
    out = [n for n in names if (m := rx.match(n)) and m.group(1) in drop]
    # Each build's checksum files are named SHA256SUMS-<version> (and .sig); drop those of old builds too.
    out += [n for n in sums if any(n.endswith(v) or n.endswith(v + ".sig") for v in drop)]
    return out


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    a = sub.add_parser("nightly-version")
    a.add_argument("--date", required=True)
    a.add_argument("--sha", required=True)
    a.add_argument("--tags")
    a = sub.add_parser("tag-info")
    a.add_argument("tag")
    a = sub.add_parser("check-tag")
    a.add_argument("tag")
    a = sub.add_parser("package")
    a.add_argument("--binary", required=True)
    a.add_argument("--target", required=True)
    a.add_argument("--version", required=True)
    a.add_argument("--out", required=True)
    a = sub.add_parser("sums")
    a.add_argument("dir")
    a = sub.add_parser("notes")
    a.add_argument("--to", required=True)
    a.add_argument("--from", dest="from_tag")
    a = sub.add_parser("prune-nightlies")
    a.add_argument("--names", required=True)
    a.add_argument("--keep", type=int, default=14)
    args = ap.parse_args()

    if args.cmd == "nightly-version":
        tags = open(args.tags).read().split() if args.tags else []
        print(nightly_version(args.date, args.sha, tags))
    elif args.cmd == "tag-info":
        for k, v in tag_info(args.tag).items():
            print(f"{k}={v}")
    elif args.cmd == "check-tag":
        info = tag_info(args.tag)
        if info["base"] != cargo_version():
            sys.exit(f"{args.tag}: compiler/Cargo.toml says {cargo_version()}, expected {info['base']}")
        print("ok")
    elif args.cmd == "package":
        print(package(args.binary, args.target, args.version, args.out))
    elif args.cmd == "sums":
        for n in write_sums(args.dir):
            print(n)
    elif args.cmd == "notes":
        print(notes(args.to, args.from_tag))
    elif args.cmd == "prune-nightlies":
        for n in prune(open(args.names).read().split(), args.keep):
            print(n)


if __name__ == "__main__":
    main()
