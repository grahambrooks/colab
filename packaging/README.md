# npm and PyPI packages

`colab-codemod` on npm and PyPI runs colab without a Rust toolchain:

```sh
npx colab-codemod fix --check          # or: npm install -g colab-codemod; colab …
uvx colab-codemod refactor -e '…' .    # or: pip install colab-codemod; colab …
```

The name is `colab-codemod` because `colab` is taken on both registries;
installed, the command is `colab` (and `colab-codemod`, which runs it).

## How they are built

[`build.py`](build.py) repacks release archives; nothing is compiled
there.

- **npm**: one package per platform (`colab-codemod-linux-x64`, …, with
  `os` / `cpu` set, holding the binary) and a launcher, `colab-codemod`,
  that lists them as optional dependencies, so npm installs only the
  matching one, and runs it.
- **PyPI**: one wheel per platform, `py3-none-<platform>`, installing the
  binary as the `colab` script plus a `colab-codemod` entry point.
- **Linux** gets static musl binaries: the release's glibc builds need the
  glibc of the runner that built them (2.39 on ubuntu-latest), which most
  CI images and sandboxes lack. `build.py` prefers a `*-linux-musl`
  archive when one is present and tags its wheel for both manylinux and
  musllinux; a glibc binary's wheel is tagged with the glibc it needs, so
  pip refuses it on an older system rather than installing something that
  cannot start.
- **macOS** packages hold the release's own archives, checked against its
  `SHA256SUMS`.

[`.github/workflows/packages.yml`](../.github/workflows/packages.yml)
runs after each release: it builds the musl binaries, runs `build.py`,
installs every package with pip, uvx, npm and npx on Linux x64, Linux
arm64 and macOS, and runs it. It is separate from the release workflow,
so a failure here never holds back the GitHub release or the Homebrew
formula. Run it by hand (Actions → packages) to package any release tag.

## Publishing

Off until enabled, per registry:

- **npm**: set the repository variable `PUBLISH_NPM` to `true` and the
  secret `NPM_TOKEN` to an automation token that can publish
  `colab-codemod` and `colab-codemod-*`.
- **PyPI**: set `PUBLISH_PYPI` to `true`, create a `pypi` environment,
  and add this repository's `packages.yml` as a trusted publisher of
  `colab-codemod` on PyPI.

A version already on a registry is skipped, so a re-run is safe.

## Locally

```sh
gh release download v2026.11.3 -D dist
python3 packaging/build.py v2026.11.3 dist out
pip install out/pypi/colab_codemod-*-macosx_11_0_arm64.whl
python3 -m unittest discover -s packaging      # the tests CI runs
```
