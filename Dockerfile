# 0721 resource service — multi-stage build.
#
# Stage 1 compiles the workspace (the placeholder static site ships from
# the repo; the hikari webui wave adds its own build stage and switches
# the server to embedded-frontend).
FROM rust:1 AS stage-build

# libwebp (the `webp` crate builds its C sources via cc) and the image
# codecs need a C toolchain; the rust image carries gcc already.
RUN apt update && apt install -y --no-install-recommends clang && rm -rf /var/lib/apt/lists/*

WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY packages ./packages
COPY res ./res
RUN cargo build --release --locked --package i0721-server

# Stage 2 keeps only the binary, the placeholder site and the languages.
FROM debian:bookworm-slim AS stage-runtime

# curl stays in the runtime image: the compose healthcheck probes
# /api/health through it.
RUN apt update && apt install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --create-home --uid 1000 i0721

COPY --from=stage-build /build/target/release/i0721-server /app/i0721-server
COPY res/website /app/res/website
COPY res/languages /app/res/languages

# Writable blob root (mount a volume here; compose uses a named volume).
RUN mkdir -p /app/data/media /app/data/media-cache && chown -R i0721:i0721 /app

USER i0721
WORKDIR /app
ENV I0721_MEDIA_ROOT=/app/data \
    I0721_STATIC_DIR=/app/res/website
EXPOSE 80
ENTRYPOINT ["./i0721-server"]
