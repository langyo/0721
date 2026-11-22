# 0721 — image hosting / netdisk resource service.
#
# Shared celestia-devtools recipes — NOT in git. Stage with: just fetch.
# `import?` silently skips when absent, so this justfile parses pre-fetch.
import? "./.just/celestia-devtools.just"

set shell := ["bash", "-c"]
set windows-shell := ["bash.exe", "-c"]

# The staged celestia-devtools recipes (0.14.2) use list functions.
set unstable
set lists

# ── web UI (packages/webui, pnpm workspace) ─────────────────────────────
#
# The server's default `embedded-frontend` feature include_dir!s
# ../../dist/webui at COMPILE time, so every rust recipe that builds the
# server crate needs the dist to exist. `webui` (re)builds it; a fresh
# checkout without node can pass `--no-default-features` for a pure-API
# loop.

# Build the web UI into dist/webui (vue-tsc + check_i18n + vite, with the
# bare-specifier output gate).
webui:
    pnpm --filter @celestia-island/i0721-webui build

# The web UI test lane (vitest; vue-tsc runs in `webui`).
webui-test:
    pnpm --filter @celestia-island/i0721-webui test

# Serve the built dist/webui locally (static preview of the SPA).
webui-serve:
    pnpm --filter @celestia-island/i0721-webui exec vite preview --outDir ../../dist/webui --port 4173

# Regenerate the Rust→TS bindings into packages/webui/src/types. Run
# after changing an exported proto/server DTO, then commit the diff
# (the generated files live in the tree; never hand-write them).
webui-types:
    cargo test -p i0721-proto -p i0721-server --quiet

# Fail when the committed TS bindings drifted from the Rust definitions:
# regenerate and git-diff. CI + gate both pin this so a stale contract
# can never merge.
webui-types-check: webui-types
    git diff --exit-code -- packages/webui/src/types

# ── rust ────────────────────────────────────────────────────────────────

# Type-check all targets (needs dist/webui for the embedded build —
# `just webui` first on a fresh checkout).
check: webui
    cargo check --workspace --all-targets

# Lint with clippy (deny warnings).
lint: webui
    cargo clippy --workspace --all-targets -- -D warnings

# Run all tests (no external services: the PostgreSQL lane self-skips).
test: webui
    cargo test --workspace

# Opt-in PostgreSQL lane (`pg_*` tests in i0721-store). Fail-closed:
# with I0721_TEST_PG set, an unreachable database or failed migration
# panics instead of skipping. Point I0721_DATABASE_URL at a SCRATCH
# database (e.g. createdb i0721_dev_$(date +%Y%m%d)); tests leave a few
# lane_* rows behind, so never aim it at production.
pg-test:
    I0721_TEST_PG=1 cargo test --workspace pg_ -- --nocapture

# Format check.
fmt:
    cargo fmt --all -- --check

# Format write.
fmt-write:
    cargo fmt --all

# Dependency licences: permissive only, no AGPL/GPL/LGPL in the tree.
licenses:
    cargo deny check licenses bans sources

# The i18n completeness gate (missing keys / undefined references /
# placeholder parity; en + zh-Hans baselines).
i18n-check:
    python3 scripts/check_i18n.py --quiet

# One-shot gate, both sides: rust (fmt -> clippy -> test -> deny) with
# the embedded dist built first, then the webui lane (vitest) and the
# TS-contract staleness pin.
gate:
    just webui
    just fmt
    just lint
    just test
    just licenses
    just webui-test
    just webui-types-check
    just i18n-check
