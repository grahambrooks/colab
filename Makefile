# Standard targets. `make check` is the pre-push gate and runs exactly what the
# CI check job runs (.github/workflows/ci.yml); `make hooks` installs the git hooks.
.DEFAULT_GOAL := help

CARGO ?= cargo

.PHONY: help check fmt fmt-check lint test licenses licenses-check hooks install

# Where `make install` puts the binary: cargo's own root (~/.cargo/bin) unless
# set, e.g. `make install INSTALL_ROOT=/usr/local`.
INSTALL_ROOT ?=

help: ## List targets
	@awk 'BEGIN {FS = ":.*?## "} /^[a-zA-Z_-]+:.*?## / {printf "  \033[36m%-16s\033[0m %s\n", $$1, $$2}' $(MAKEFILE_LIST)

check: fmt-check lint test doc self-check licenses-check ## The pre-push gate: the same commands as the CI check job

# CI sets RUSTFLAGS=-D warnings for the whole workflow, and RUSTDOCFLAGS for
# `cargo doc`.
check lint test doc self-check: export RUSTFLAGS = -D warnings
check doc: export RUSTDOCFLAGS = -D warnings

doc: ## Build the docs, warnings (broken intra-doc links) as errors
	$(CARGO) doc --workspace --no-deps --all-features --locked

self-check: ## colab on itself, as CI runs it: pack cases, colab.toml's packs, Markdown links
	$(CARGO) run -q -p colab-cli --locked -- test
	$(CARGO) run -q -p colab-cli --locked -- fix --check
	$(CARGO) run -q -p colab-cli --locked -- links docs

fmt: ## Format in place
	$(CARGO) fmt --all

fmt-check: ## Check formatting
	$(CARGO) fmt --all --check

lint: ## Clippy, warnings as errors
	$(CARGO) clippy --workspace --all-targets --all-features --locked -- -D warnings

test: ## Run the tests
	$(CARGO) test --workspace --all-features --locked

licenses: ## Regenerate THIRD-PARTY-LICENSES.txt (after changing dependencies)
	scripts/third-party-licenses.py

licenses-check: ## Fail when THIRD-PARTY-LICENSES.txt is stale
	scripts/third-party-licenses.py --check

hooks: ## Install the git hooks (prek: pre-commit and pre-push)
	prek install

install: ## Build colab in release mode and install it (INSTALL_ROOT=<dir> for <dir>/bin)
	$(CARGO) install --path crates/colab-cli --locked --force $(if $(INSTALL_ROOT),--root $(INSTALL_ROOT))
	@command -v colab >/dev/null && echo "colab on PATH: $$(command -v colab) ($$(colab --version))"

include release.mk
