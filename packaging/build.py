#!/usr/bin/env python3
"""Repack colab's release archives as npm packages and Python wheels.

    python3 packaging/build.py VERSION DIST OUT

DIST holds the release assets (`colab-vVERSION-<target>.tar.gz`, and
`SHA256SUMS` when present, which is then checked). OUT receives:

    OUT/npm/colab-codemod/                 launcher: `colab` and `colab-codemod`
    OUT/npm/colab-codemod-<os>-<cpu>/      one per target, the binary itself
    OUT/pypi/colab_codemod-VERSION-py3-none-<platform>.whl

Nothing is compiled: each package holds the binary the release built, so
`npx colab-codemod` and `uvx colab-codemod` run the same bytes as the
Homebrew formula. Targets missing from DIST are skipped (with a note),
so a local run can package just the host. Only the standard library is
used, so the release runner needs nothing installed.
"""
from __future__ import annotations

import base64
import csv
import hashlib
import io
import json
import re
import shutil
import sys
import tarfile
import zipfile
from pathlib import Path

NAME = "colab-codemod"
BIN = "colab"
REPO = "https://github.com/grahambrooks/colab"
DESCRIPTION = "Syntax-aware codemods for 12 languages and TOML / JSON / Markdown config"
LICENSE = "MIT"

# Rust target -> (npm os, npm cpu, wheel platform; None: from the binary).
# For each os/cpu the first target present wins: a static musl binary runs
# on any Linux, while the release's glibc build needs the glibc it was
# built against (see `linux_tag`).
TARGETS = {
    "aarch64-apple-darwin": ("darwin", "arm64", "macosx_11_0_arm64"),
    "x86_64-apple-darwin": ("darwin", "x64", "macosx_10_12_x86_64"),
    "x86_64-unknown-linux-musl": ("linux", "x64", None),
    "x86_64-unknown-linux-gnu": ("linux", "x64", None),
    "aarch64-unknown-linux-musl": ("linux", "arm64", None),
    "aarch64-unknown-linux-gnu": ("linux", "arm64", None),
}

LAUNCHER = r"""#!/usr/bin/env node
// Runs the colab binary from the platform package npm installed beside
// this one (an optional dependency, so only the matching one installs).
"use strict";
const { spawnSync } = require("child_process");
const fs = require("fs");

const PLATFORMS = __PLATFORMS__;
const key = `${process.platform}-${process.arch}`;
const pkg = PLATFORMS[key];
if (!pkg) {
  console.error(
    `colab: no prebuilt binary for ${key}; build it with ` +
      "`cargo install --git __REPO__ colab-cli`"
  );
  process.exit(1);
}
let bin;
try {
  bin = require.resolve(`${pkg}/bin/__BIN__`);
} catch {
  console.error(
    `colab: ${pkg} is not installed. npm skips optional dependencies with ` +
      "--omit=optional or --no-optional; reinstall without it."
  );
  process.exit(1);
}
try {
  fs.accessSync(bin, fs.constants.X_OK);
} catch {
  try { fs.chmodSync(bin, 0o755); } catch {}
}
const run = spawnSync(bin, process.argv.slice(2), { stdio: "inherit" });
if (run.error) {
  console.error(`colab: ${run.error.message}`);
  process.exit(1);
}
if (run.signal) {
  process.kill(process.pid, run.signal);
}
process.exit(run.status ?? 1);
"""

PY_INIT = '''"""Runs the colab binary this wheel installed (`colab-codemod`, or
`python -m colab_codemod`); the binary itself is installed as `colab`."""
import os
import sys
import sysconfig


def _binary():
    name = "colab.exe" if os.name == "nt" else "colab"
    dirs = [sysconfig.get_path("scripts"), os.path.dirname(sys.executable)]
    user = sysconfig.get_path("scripts", f"{os.name}_user")
    if user:
        dirs.append(user)
    for d in dirs:
        path = os.path.join(d, name)
        if os.path.isfile(path):
            return path
    raise SystemExit("colab: the colab binary is missing from " + ", ".join(dirs))


def main():
    path = _binary()
    if os.name == "nt":
        import subprocess
        raise SystemExit(subprocess.call([path, *sys.argv[1:]]))
    os.execv(path, [path, *sys.argv[1:]])
'''


def npm_name(os_: str, cpu: str) -> str:
    return f"{NAME}-{os_}-{cpu}"


def read_binary(archive: Path) -> bytes:
    with tarfile.open(archive) as tar:
        member = next((m for m in tar.getmembers() if Path(m.name).name == BIN and m.isfile()), None)
        if member is None:
            raise SystemExit(f"{archive}: no `{BIN}` binary inside")
        return tar.extractfile(member).read()


def check_sums(dist: Path, archives: dict[str, Path]) -> None:
    sums = dist / "SHA256SUMS"
    if not sums.exists():
        print("note: no SHA256SUMS; archives not checked", file=sys.stderr)
        return
    want = {}
    for line in sums.read_text().splitlines():
        if line.strip():
            digest, name = line.split(maxsplit=1)
            want[name.lstrip("*")] = digest
    for archive in archives.values():
        if archive.name not in want:
            print(f"note: {archive.name} is not in SHA256SUMS; not checked", file=sys.stderr)
            continue
        got = hashlib.sha256(archive.read_bytes()).hexdigest()
        if want[archive.name] != got:
            raise SystemExit(f"{archive.name}: SHA256 does not match SHA256SUMS")


def linux_tag(binary: bytes, arch: str) -> str:
    """The wheel platform for a Linux binary: manylinux for the newest glibc
    symbol version it needs, or, for a static binary that needs none, both
    manylinux and musllinux, since it runs on either."""
    minors = [int(m.group(1)) for m in re.finditer(rb"GLIBC_2\.(\d+)", binary)]
    if not minors:
        return f"manylinux_2_17_{arch}.musllinux_1_1_{arch}"
    return f"manylinux_2_{max(minors + [17])}_{arch}"


def write_json(path: Path, value: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2) + "\n")


def npm_packages(version: str, binaries: dict[str, bytes], out: Path, readme: str) -> None:
    root = out / "npm"
    platforms = {}
    for target, binary in binaries.items():
        os_, cpu, _ = TARGETS[target]
        name = npm_name(os_, cpu)
        platforms[f"{os_}-{cpu}"] = name
        pkg = root / name
        (pkg / "bin").mkdir(parents=True, exist_ok=True)
        exe = pkg / "bin" / BIN
        exe.write_bytes(binary)
        exe.chmod(0o755)
        (pkg / "README.md").write_text(f"The `{BIN}` binary for {os_} {cpu}, used by [`{NAME}`]({REPO}).\n")
        write_json(pkg / "package.json", {
            "name": name,
            "version": version,
            "description": f"{DESCRIPTION} ({os_} {cpu} binary)",
            "license": LICENSE,
            "repository": {"type": "git", "url": f"git+{REPO}.git"},
            "os": [os_],
            "cpu": [cpu],
            "files": ["bin/", "README.md"],
            "preferUnplugged": True,
        })
    launcher = root / NAME
    (launcher / "bin").mkdir(parents=True, exist_ok=True)
    script = (
        LAUNCHER.replace("__PLATFORMS__", json.dumps(dict(sorted(platforms.items()))))
        .replace("__REPO__", REPO)
        .replace("__BIN__", BIN)
    )
    (launcher / "bin" / "colab.js").write_text(script)
    (launcher / "bin" / "colab.js").chmod(0o755)
    (launcher / "README.md").write_text(readme)
    write_json(launcher / "package.json", {
        "name": NAME,
        "version": version,
        "description": DESCRIPTION,
        "license": LICENSE,
        "repository": {"type": "git", "url": f"git+{REPO}.git"},
        "homepage": REPO,
        "keywords": ["codemod", "refactoring", "tree-sitter", "mcp", "lsp"],
        "bin": {BIN: "bin/colab.js", NAME: "bin/colab.js"},
        "files": ["bin/", "README.md"],
        "optionalDependencies": {name: version for name in sorted(platforms.values())},
    })


def record_hash(data: bytes) -> str:
    return "sha256=" + base64.urlsafe_b64encode(hashlib.sha256(data).digest()).rstrip(b"=").decode()


def wheel(version: str, platform: str, binary: bytes, out: Path, readme: str) -> Path:
    dist = NAME.replace("-", "_")
    info = f"{dist}-{version}.dist-info"
    files: list[tuple[str, bytes, int]] = [
        (f"{dist}/__init__.py", PY_INIT.encode(), 0o644),
        (f"{dist}/__main__.py", b"from colab_codemod import main\n\nmain()\n", 0o644),
        (f"{dist}-{version}.data/scripts/{BIN}", binary, 0o755),
        (f"{info}/METADATA", (
            "Metadata-Version: 2.1\n"
            f"Name: {NAME}\n"
            f"Version: {version}\n"
            f"Summary: {DESCRIPTION}\n"
            f"Home-page: {REPO}\n"
            f"License: {LICENSE}\n"
            "Requires-Python: >=3.8\n"
            "Description-Content-Type: text/markdown\n"
            "\n" + readme
        ).encode(), 0o644),
        (f"{info}/WHEEL", (
            "Wheel-Version: 1.0\n"
            "Generator: colab packaging/build.py\n"
            "Root-Is-Purelib: false\n"
            f"Tag: py3-none-{platform}\n"
        ).encode(), 0o644),
        (f"{info}/entry_points.txt", f"[console_scripts]\n{NAME} = {dist}:main\n".encode(), 0o644),
    ]
    record = io.StringIO()
    writer = csv.writer(record, lineterminator="\n")
    for name, data, _ in files:
        writer.writerow([name, record_hash(data), len(data)])
    writer.writerow([f"{info}/RECORD", "", ""])
    files.append((f"{info}/RECORD", record.getvalue().encode(), 0o644))

    path = out / "pypi" / f"{dist}-{version}-py3-none-{platform}.whl"
    path.parent.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as zf:
        for name, data, mode in files:
            entry = zipfile.ZipInfo(name, date_time=(2020, 1, 1, 0, 0, 0))
            entry.external_attr = (0o100000 | mode) << 16
            entry.compress_type = zipfile.ZIP_DEFLATED
            zf.writestr(entry, data)
    return path


README = f"""# {NAME}

[colab]({REPO}) — syntax-aware codemods for C, C++, C#, Go, Java,
JavaScript/TypeScript, Kotlin, PHP, Python, Ruby, Rust and Swift, and
structured edits to TOML, JSON and Markdown, with a CLI, an MCP server
and an LSP.

```sh
npx colab-codemod refactor -e "match go::import 'a' {{ replace 'b' }}" --format diff .
uvx colab-codemod fix --check
```

This package holds the prebuilt `{BIN}` binary from colab's GitHub
release; installed, the command is `{BIN}`. See the
[documentation]({REPO}/tree/main/docs).
"""


def main(argv: list[str]) -> int:
    if len(argv) != 4:
        print(__doc__, file=sys.stderr)
        return 2
    version, dist, out = argv[1].lstrip("v"), Path(argv[2]), Path(argv[3])
    archives = {}
    chosen = set()
    for target, (os_, cpu, _) in TARGETS.items():
        archive = dist / f"{BIN}-v{version}-{target}.tar.gz"
        if (os_, cpu) in chosen:
            continue
        if archive.exists():
            archives[target] = archive
            chosen.add((os_, cpu))
    for os_, cpu in sorted({(o, c) for o, c, _ in TARGETS.values()} - chosen):
        print(f"note: no archive for {os_} {cpu}; skipping it", file=sys.stderr)
    if not archives:
        raise SystemExit(f"no release archives for {version} in {dist}")
    check_sums(dist, archives)
    binaries = {target: read_binary(path) for target, path in archives.items()}
    if out.exists():
        shutil.rmtree(out)
    npm_packages(version, binaries, out, README)
    for target, binary in binaries.items():
        _, _, platform = TARGETS[target]
        if platform is None:
            platform = linux_tag(binary, target.split("-")[0])
        print(wheel(version, platform, binary, out, README))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
