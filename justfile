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

# Regenerate the Rust→TS bindings AND the types barrel into
# packages/webui/src/types. Run after changing an exported proto/server
# DTO, then commit the diff (the generated files live in the tree; never
# hand-write them). The barrel is derived from the binding files on disk
# (scripts/gen_types_barrel.py), so a new #[ts(export)] can never land
# as a file the barrel silently fails to re-export.
webui-types:
    cargo test -p i0721-proto -p i0721-server --quiet
    python3 scripts/gen_types_barrel.py

# Fail when the committed BARREL drifted: bijection against the disk
# binding files + content check, with NO regeneration anywhere in the
# recipe — a hand-edited or stale index.ts fails fast with the precise
# missing/extra line instead of being silently rewritten back to health.
webui-types-check:
    python3 scripts/gen_types_barrel.py --check

# Binding staleness is a SEPARATE lane: the ts-rs export tests inside
# `cargo test --workspace` regenerate every binding, and the git diff
# then fails on any committed binding that no longer matches the Rust
# definitions. (Depends on `test` so the gate never runs cargo twice.)
webui-bindings-check: test
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
# Serialised on purpose: the migration tests drop/recreate the global
# unique index on a shared scratch database, so parallel index tests race
# each other's hand-planted rows (11 failures / 26 parallel runs measured).
pg-test:
    I0721_TEST_PG=1 cargo test --workspace pg_ -- --nocapture --test-threads=1

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

# Verify the pnpm lockfile chain in one shot: the declarations
# (typescript `^*` etc.) ↔ the workspace override (`~6.0` — without the
# override the bare `^*` specifier cannot resolve) ↔ the committed
# lockfile. Frozen + lockfile-only: no writes, no network fetches of
# tarballs, fails the moment any link of the chain drifts.
webui-lockfile-check:
    pnpm install --frozen-lockfile --lockfile-only

# Opt-in S3 real-endpoint lane (needs a reachable minio/R2 + S3 env).
s3-test:
    I0721_TEST_S3=1 cargo test -p i0721-server --test routes s3_real -- --ignored --nocapture

# One-shot gate, both sides: rust (fmt -> clippy -> test+bindings
# staleness -> deny) with the embedded dist built first, then the webui
# lane (vitest), the barrel bijection pin, the lockfile chain and the
# i18n gate.
gate:
    just webui
    just fmt
    just lint
    just webui-bindings-check
    just licenses
    just webui-test
    just webui-types-check
    just webui-lockfile-check
    just i18n-check
