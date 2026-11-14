#!/usr/bin/env python3
"""Estimate what a diff would have cost an agent as Read + Edit calls.

Feed it the change colab made, as a zero-context diff, and the revision
the change was made against:

    git diff -U0 > change.diff
    scripts/price-edits.py --rev HEAD change.diff

It models the cheapest realistic route to the same diff: one grep to find
the sites, one targeted Read (the hunks plus CONTEXT lines either side,
with line-number prefixes) per file, since Edit refuses an unread file,
and one Edit per hunk carrying an anchor line for uniqueness. It prints
bytes, and tokens as bytes / 4. Compare it with the bytes of the colab
commands and their output (see docs/dogfood-log.md, "Token cost").
"""

import argparse
import re
import subprocess
import sys

EDIT_OVERHEAD = 150  # tool name, path, JSON keys
EDIT_RESULT = 110    # "The file … has been updated successfully."
READ_CALL = 150
LINE_PREFIX = 7      # cat -n numbering
CONTEXT = 12
GREP_CALL, GREP_LINE = 200, 110


def parse(diff):
    files, current = {}, None
    for line in diff.splitlines():
        if line.startswith("+++ b/"):
            current = line[6:].strip()
            files.setdefault(current, [])
        elif line.startswith("@@") and current:
            m = re.match(r"@@ -(\d+)(?:,(\d+))? \+\d+(?:,\d+)? @@", line)
            files[current].append([int(m[1]), int(m[2] or 1), [], []])
        elif current and files[current] and line.startswith("-") and not line.startswith("---"):
            files[current][-1][2].append(line[1:] + "\n")
        elif current and files[current] and line.startswith("+") and not line.startswith("+++"):
            files[current][-1][3].append(line[1:] + "\n")
    return files


def price(files, repo, rev):
    edit = read = hunks = 0
    for path, hs in files.items():
        shown = subprocess.run(["git", "-C", repo, "show", f"{rev}:{path}"],
                               capture_output=True, text=True)
        src = shown.stdout.splitlines(True)
        lines = set()
        for start, count, old, new in hs:
            lines.update(range(max(0, start - 1 - CONTEXT),
                               min(len(src), start - 1 + max(count, 1) + CONTEXT)))
            anchor = src[start - 2] if 2 <= start <= len(src) + 1 else ""
            edit += EDIT_OVERHEAD + 2 * len(anchor) + sum(map(len, old)) + sum(map(len, new))
            edit += EDIT_RESULT
            hunks += 1
        read += READ_CALL + sum(len(src[i]) + LINE_PREFIX for i in lines)
    grep = GREP_CALL + GREP_LINE * hunks
    return {"files": len(files), "hunks": hunks, "grep": grep, "read": read,
            "edit": edit, "total": grep + read + edit}


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("diff", nargs="?", help="zero-context diff (default: stdin)")
    ap.add_argument("--rev", default="HEAD", help="revision the diff applies to")
    ap.add_argument("-C", dest="repo", default=".", help="repository")
    args = ap.parse_args()
    text = open(args.diff).read() if args.diff else sys.stdin.read()
    r = price(parse(text), args.repo, args.rev)
    print(f"{r['hunks']} hunk(s) in {r['files']} file(s): grep {r['grep']} + read {r['read']}"
          f" + edit {r['edit']} = {r['total']} bytes (~{r['total'] // 4} tokens)")


if __name__ == "__main__":
    main()
