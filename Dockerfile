# 0721 resource service — multi-stage build.
#
# Stage 1 builds the web UI (node), stage 2 compiles the workspace with
# the dist embedded (cargo --features embedded-frontend, on by default),
# stage 3 keeps the runtime lean. The frontend → cargo order is the
# contract: the embedded build literally include_dir!s the dist the node
# stage produced (the chest four-guardrail Dockerfile shape: the
# bare-specifier gate and the vue-tsc/check_i18n runs all happen inside
# the webui build command before any byte reaches the binary).
#
# Base image refs are overridable build args so CI can retarget them at a
# registry mirror when docker.io is unreachable from the fleet (the chest
# pattern). Defaults keep plain `docker build` on the upstream names.
ARG NODE_IMAGE=node:24-slim
ARG RUST_IMAGE=rust:1-slim
ARG BASE_IMAGE=debian:bookworm-slim

FROM ${NODE_IMAGE} AS stage-webui

# python3 runs the i18n gate inside the webui build command; corepack
# ships with node and pins the pnpm version the lockfile expects.
RUN apt-get update && apt-get install -y --no-install-recommends python3 ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && corepack enable && corepack prepare pnpm@12.5.1 --activate

WORKDIR /build
COPY pnpm-workspace.yaml pnpm-lock.yaml ./
COPY packages/webui/package.json packages/webui/package.json
RUN pnpm install --frozen-lockfile

# The locale JSONs feed the webui's import.meta.glob; the committed
# ts-rs bindings (packages/webui/src/types) ship in the tree.
COPY res res
COPY packages/webui packages/webui
RUN pnpm --filter @celestia-island/i0721-webui build

FROM ${RUST_IMAGE} AS stage-build

# libwebp (the `webp` crate builds its C sources via cc) and the image
# codecs need a C toolchain; the rust image carries gcc already.
RUN apt update && apt install -y --no-install-recommends clang \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY packages ./packages
# The embedded-frontend feature (default) compiles the dist in.
COPY --from=stage-webui /build/dist ./dist
RUN cargo build --release --locked --package i0721-server

# Stage 3 keeps only the binary — the frontend is inside it.
FROM ${BASE_IMAGE} AS stage-runtime

# curl stays in the runtime image: the compose healthcheck probes
# /api/health through it.
RUN apt update && apt install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --create-home --uid 1000 i0721

COPY --from=stage-build /build/target/release/i0721-server /app/i0721-server

# Writable blob root (mount a volume here; compose uses a named volume).
RUN mkdir -p /app/data/media /app/data/media-cache && chown -R i0721:i0721 /app

USER i0721
WORKDIR /app
ENV I0721_MEDIA_ROOT=/app/data
EXPOSE 8080
ENTRYPOINT ["./i0721-server"]
