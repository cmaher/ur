# Keep targets small and composable. Multi-line logic goes in scripts/build/.
# Compatible with GNU Make 3.81 (the macOS default).

SHELL := /bin/bash
.DEFAULT_GOAL := help

.PHONY: help fmt fmt-fix fmt-fix-silent clippy build test test-init test-clean audit \
	ci ci-fmt-fix test-ci-scripts test-ci-scripts-if-changed housekeeping \
	stage stage-native \
	image image-native image-force-base image-update-claude image-update-codex image-update-agy \
	install install-nocache install-debug install-update-claude install-update-codex install-update-agy \
	tools acceptance acceptance-ci install-hooks

help: ## List targets
	@grep -E '^[a-zA-Z0-9_-]+:.*## ' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*## "}; {printf "  %-28s %s\n", $$1, $$2}'

# --- Checks -----------------------------------------------------------------

fmt: ## Auto-fix formatting, then fail if changes were needed
	scripts/build/fmt-check.sh

fmt-fix: ## Fix code formatting
	cargo fmt --all

fmt-fix-silent: ## Fix code formatting (no output)
	@cargo fmt --all >/dev/null 2>&1

clippy: ## Run clippy lints on all workspace crates
	cargo clippy --locked --workspace --all-targets --all-features -- -D warnings

# Same features and targets as `test`, so `test` reuses every artifact instead
# of compiling the workspace a second time. --all-targets builds test harnesses
# too (activating dev-dependency features); `ur`/`builderd` binaries in
# target/debug are what the acceptance suite runs.
build: ## Build all workspace crates and test targets
	cargo build --workspace --all-targets

test: test-init ## Run tests on all workspace crates
	cargo test --workspace

test-init: ## Start ci-postgres container for tests (port 5433) if not already up
	@scripts/build/test-init.sh

test-clean: ## Stop and remove ci-postgres container
	-@docker stop ci-postgres >/dev/null 2>&1
	-@docker rm ci-postgres >/dev/null 2>&1
	@echo "ci-postgres removed"

audit: ## Run cargo audit for dependency vulnerabilities
	cargo audit

ci: fmt test-ci-scripts-if-changed clippy build test ## Run all CI checks

ci-fmt-fix: fmt-fix-silent clippy build test ## Auto-fix formatting silently, then run CI checks (clippy, build, test)

test-ci-scripts: ## Test the CI shell layer (image hash, acceptance-ci, stage, housekeeping, pre-push) in a stubbed sandbox
	bash scripts/build/test-ci-scripts.sh

# Part of `ci`: runs only when the CI scripts differ from origin/master
# (UR_CI_TEST_FORCE=1 forces it). `make test-ci-scripts` always runs.
test-ci-scripts-if-changed: ## Run test-ci-scripts if CI scripts, hooks or the Makefile changed since origin/master
	bash scripts/build/test-ci-scripts.sh --if-changed

housekeeping: ## Prune stale target/ artifacts (cargo-sweep) and cap Docker's build cache
	scripts/build/housekeeping.sh

# --- Staging ----------------------------------------------------------------

stage: ## Build image binaries (ur-server, workerd, ur-workertools) in one cross-compile and stage them
	scripts/build/stage.sh

stage-native: ## Same as stage but with a native (non-zig) build (Linux hosts only)
	UR_NATIVE_BUILD=1 scripts/build/stage.sh

# --- Images -----------------------------------------------------------------

image: stage ## Build all container images (worker + server)
	scripts/build/image.sh

image-native: stage-native ## Build all container images using native compilation (Linux CI)
	scripts/build/image.sh

image-force-base: stage ## Build all images, force-rebuilding the base image (no Docker cache)
	UR_FORCE_REBUILD_BASE=1 scripts/build/image.sh

image-update-claude: stage ## Build all images, forcing a Claude Code update in the worker image
	UR_UPDATE_AGENT=claude scripts/build/image.sh

image-update-codex: stage ## Build all images, forcing a codex update in the worker image
	UR_UPDATE_AGENT=codex scripts/build/image.sh

image-update-agy: stage ## Build all images, forcing an AGY update in the worker image
	UR_UPDATE_AGENT=agy scripts/build/image.sh

# --- Install ----------------------------------------------------------------

install: image ## Build ur CLI + builderd + ur-workertools, install to UR_INSTALL_DIR, build all images
	scripts/build/install.sh

install-nocache: image-force-base ## Same as install but force-rebuilds the base image (no Docker cache)
	scripts/build/install.sh

install-debug: image ## Same as install but with debug builds
	UR_BUILD_PROFILE=debug scripts/build/install.sh

install-update-claude: image-update-claude ## Same as install but forces a Claude Code update
	scripts/build/install.sh

install-update-codex: image-update-codex ## Same as install but forces a codex update
	scripts/build/install.sh

install-update-agy: image-update-agy ## Same as install but forces an AGY update
	scripts/build/install.sh

tools: ## Build and install tool binaries (tk-import, tk-verify) to UR_INSTALL_DIR
	scripts/build/install-tools.sh

# --- Acceptance -------------------------------------------------------------

acceptance: image ## Build container images and run acceptance tests
	cargo test -p acceptance --features acceptance

acceptance-ci: ## Build content-hash-tagged CI images (skipped when already built) and run acceptance tests
	scripts/acceptance-ci.sh

install-hooks: ## Install git hooks (pre-commit formats, pre-push runs CI + acceptance)
	scripts/build/install-hooks.sh
