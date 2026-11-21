# 0721 — image hosting / netdisk resource service.
#
# Shared celestia-devtools recipes — NOT in git. Stage with: just fetch.
# `import?` silently skips when absent, so this justfile parses pre-fetch.
import? "./.just/celestia-devtools.just"

set shell := ["bash", "-c"]
set windows-shell := ["bash.exe", "-c"]

# The staged celestia-devtools recipes (0.7.0) use list functions.
set unstable
set lists

# Type-check all targets.
check:
    cargo check --workspace --all-targets

# Lint with clippy (deny warnings).
lint:
    cargo clippy --workspace --all-targets -- -D warnings

# Run all tests (no external services: the PostgreSQL lane self-skips).
test:
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

# One-shot gate (fmt -> clippy -> test -> deny).
gate: fmt lint test licenses
