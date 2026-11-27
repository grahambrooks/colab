#!/usr/bin/env python3
"""Fetch the wide corpora colab's parsers are measured on, and run a
language's comparison over its corpus.

    scripts/parser-corpora.py fetch [<lang> ...]   # clone at the recorded commits
    scripts/parser-corpora.py run <lang> [--show N]  # agreement with tree-sitter

The corpora are listed, with their commits, in tests/parsers/corpora.toml.
Clones go to ~/.cache/colab-corpora/<lang>/<name> (shallow, one commit
each) and are reused. `run` prints the comparison's summary line: files,
those tree-sitter reads cleanly, how many trees are the same, and how many
clean files colab reads with an error. Agreement is a report, not a gate:
colab's parsers read each language's own grammar where tree-sitter
misreads it.
"""

import os
import pathlib
import subprocess
import sys
import tomllib

ROOT = pathlib.Path(__file__).resolve().parent.parent
MANIFEST = ROOT / "tests" / "parsers" / "corpora.toml"
CACHE = pathlib.Path.home() / ".cache" / "colab-corpora"


def languages() -> dict:
    with MANIFEST.open("rb") as f:
        return tomllib.load(f)["languages"]


def fetch(names: list[str]) -> None:
    langs = languages()
    for lang in names or sorted(langs):
        spec = langs[lang]
        if "local" in spec:
            print(f"{lang}: measured over {spec['local']}; nothing to fetch")
            continue
        for repo in spec["repos"]:
            dest = CACHE / lang / repo["name"]
            head = subprocess.run(
                ["git", "-C", str(dest), "rev-parse", "HEAD"],
                capture_output=True, text=True,
            ).stdout.strip() if (dest / ".git").is_dir() else ""
            if head == repo["commit"]:
                continue
            dest.mkdir(parents=True, exist_ok=True)
            git = ["git", "-C", str(dest)]
            if not (dest / ".git").is_dir():
                subprocess.run(git + ["init", "-q"], check=True)
                subprocess.run(git + ["remote", "add", "origin", repo["url"]], check=True)
            print(f"{lang}/{repo['name']} @ {repo['commit'][:12]}", flush=True)
            subprocess.run(git + ["fetch", "-q", "--depth", "1", "origin", repo["commit"]], check=True)
            subprocess.run(git + ["checkout", "-q", "--detach", "FETCH_HEAD"], check=True)


def run(lang: str, show: int) -> int:
    spec = languages()[lang]
    if "local" in spec:
        print(f"{lang} was measured over {spec['local']}: set {spec['env']} to a directory and run "
              f"`cargo test --release -p colab-backends --test {spec['test']} -- --nocapture`")
        return 1
    corpus = CACHE / lang
    if not corpus.is_dir():
        print(f"no corpus for {lang}: run `scripts/parser-corpora.py fetch {lang}` first")
        return 1
    show_var = spec["env"].replace("_FILES", "_SHOW")
    env = dict(os.environ, **{spec["env"]: str(corpus), show_var: str(show)},
               OTEL_SDK_DISABLED="true")
    # The C and C++ comparisons share a test, as JavaScript's and
    # TypeScript's do; the other language's run reads the repo only.
    out = subprocess.run(
        ["cargo", "test", "-q", "--release", "-p", "colab-backends", "--test", spec["test"],
         "--", "--nocapture"],
        cwd=ROOT, env=env, capture_output=True, text=True,
    )
    text = out.stdout + out.stderr
    for line in text.splitlines():
        if ("files (" in line and "clean" in line) or line.startswith("    "):
            print(line)
    return 0 if out.returncode == 0 else out.returncode


def main(argv: list[str]) -> int:
    if len(argv) < 2 or argv[1] not in ("fetch", "run"):
        print(__doc__)
        return 2
    if argv[1] == "fetch":
        fetch(argv[2:])
        return 0
    if len(argv) < 3:
        print("run needs a language")
        return 2
    show = int(argv[argv.index("--show") + 1]) if "--show" in argv else 0
    return run(argv[2], show)


if __name__ == "__main__":
    sys.exit(main(sys.argv))
