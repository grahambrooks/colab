#!/bin/sh
# Pin the current build of colab as the one used to develop colab:
# `.dogfood/colab`. Re-run after a commit that passes the tests — never
# mid-change, so a broken working tree cannot break the tool you are
# using to fix it. See docs/dogfooding.md.
set -eu
cd "$(git rev-parse --show-toplevel)"
cargo build -q -p colab-cli
target=$(cargo metadata --format-version 1 --no-deps |
    python3 -c 'import json, sys; print(json.load(sys.stdin)["target_directory"])')
mkdir -p .dogfood
cp "$target/debug/colab" .dogfood/colab.new
mv .dogfood/colab.new .dogfood/colab
printf 'pinned %s at %s\n' "$(.dogfood/colab --version)" "$(git rev-parse --short HEAD)"
