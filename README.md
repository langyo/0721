# 0721

<div align="center">
  <img src="https://raw.githubusercontent.com/celestia-island/celestia-island.github.io/main/res/logos/0721.webp" alt="0721" width="180" />
</div>

![CI workflow](https://img.shields.io/github/actions/workflow/status/langyo/0721/ci.yml)
![GitHub License](https://img.shields.io/github/license/langyo/0721)
![GitHub Tag](https://img.shields.io/github/v/tag/langyo/0721)

> Ciallo～(∠·ω< )⌒★

A simple engine for image hosting written in Rust.

> The service is being rebuilt on the Celestia island family stack
> (axum + lacrymira + hikari webui): image hosting grows into a netdisk +
> gallery with OAuth sign-in, S3/IPFS storage backends and an MCP surface
> for AI agents. The current trunk carries the rewritten data layer and
> service (PostgreSQL metadata, local-FS blobs, transition-era password +
> JWT auth); feature waves land as squash-merged pull requests.

## Features

- Image hosting
- Automatic image compression via WebP (animated GIF included)
- Netdisk-ready metadata schema (kinds, directories, soft delete, shares)
- Basic account system (admin-gated user management)
- Site configuration stored in the database — no restart to apply
- hikari web UI (Vue 3 + TypeScript): portal timeline with upload
  progress, user roster, strict site settings, theme (light/dark/auto +
  palettes) — embedded into the release binary
- Internationalization (JSON catalogs, 12 locales, en + zh-Hans baselines)
- Deploy via Docker Compose
- Written in Rust with a TypeScript frontend

## Deploy

The compose file (`deploy/docker-compose.yml`) brings up PostgreSQL 17
(health-checked) plus the service image built from the repository, with
named volumes for the database and the media blobs. Publishing is
loopback-only by default — front it with a reverse proxy or tunnel in
production.

```bash
cd deploy
cat > .env <<'EOF'
I0721_JWT_SECRET=change-me-to-a-long-random-string
POSTGRES_PASSWORD=change-me-too
EOF
docker compose up -d --build
```

> The compose variable is `I0721_JWT_SECRET` (compose interpolates it into
> the container's `JWT_SECRET`). A bare `docker run` / systemd deployment
> sets `JWT_SECRET` directly — see the table below.

The service listens on `127.0.0.1:8080` (override with
`I0721_PUBLISH=127.0.0.1:9090`). `GET /api/health` reports status and
where the site configuration came from; both containers carry
healthchecks (pg_isready + /api/health with start periods).

> **Production note**: the compose env form (secrets as environment
> variables) is for development and single-host trials. A production
> deployment passes secrets via systemd `LoadCredential=` (or an
> equivalent secret store); compose is not the production secrets
> channel.

### Single container

The image also runs standalone against an external PostgreSQL:

```bash
docker run -d -p 8080:8080 \
  -e I0721_DATABASE_URL='postgres://i0721:secret@db-host:5432/i0721' \
  -e JWT_SECRET='a-long-random-string' \
  -v i0721-media:/app/data \
  ghcr.io/langyo/0721:latest
```

Migrations apply automatically at boot (bookkeeping is compatible with
the legacy `seaql_migrations` table). On an empty user table the service
seeds an `admin` account — **the seed password is `admin` unless
`I0721_SEED_ADMIN_PASSWORD` is set, and it is only a first-boot
convenience: change it immediately** (create a second admin, then delete
or re-role the default one).

## Configuration

Process configuration is environment-only (fail-fast — an unset
required variable or an unparseable value refuses to start):

| Variable | Default | Meaning |
|---|---|---|
| `I0721_DATABASE_URL` | required | PostgreSQL URL |
| `JWT_SECRET` (container-side; compose sets it from `I0721_JWT_SECRET`) | required | HS256 secret for the transition-era tokens |
| `I0721_LISTEN` | `0.0.0.0:8080` | HTTP bind address (keep the port unprivileged: the image runs as a non-root user) |
| `I0721_MEDIA_ROOT` | `data` | blob root (`media/` and `media-cache/` below it) |
| `I0721_STATIC_DIR` | `dist/webui` | web UI dist directory (dev mode; the default build embeds the dist) |
| `I0721_UPLOAD_CONCURRENCY` | `4` | concurrent upload-pipeline slots |
| `I0721_LOG` | `i0721_server=info,tower_http=warn` | tracing filter |
| `I0721_SEED_ADMIN_PASSWORD` | `admin` | first-boot admin password (empty user table only) |
| `I0721_TRUSTED_PROXIES` | *(empty)* | CIDR list whose `X-Forwarded-For` is honored for login throttling; empty = never trust the header |

The site configuration (the old `Config.toml`) lives in the `settings`
table and is edited through the admin RPC lane (`settings.get` / `settings.put`) —
changes apply on the next request, no container restart:

```json
{
  "portal": {
    "title-suffix": "Ciallo～(∠·ω< )⌒★",
    "footer-banner": [{ "text": "© 0721 project", "url": "https://github.com/langyo/0721" }],
    "language": "zh_hans",
    "timezone": 8
  },
  "router": {
    "media-entry-path": "/media",
    "limit-referrer-host": []
  },
  "upload": {
    "image-size-limit": "8MiB",
    "webp-auto-convert": true,
    "use-source-file-name": false
  }
}
```

`media-entry-path` is pinned to `/media`: existing image links point
there and must keep working.

Two caveats worth knowing:

- Links produced by the **legacy** engine carry its own key scheme
  (base64url SHA-3-256 of the original bytes); the rewritten service
  cannot resolve them until the old data is imported — the import tool
  is a later wave, so run the generations side by side if old links
  must keep resolving during the transition.
- The referrer allow-list only stops requests whose browser sends a
  default `Referer` header. It is a courtesy gate, not technical
  protection: any client that omits the header passes it. Hard
  access control is what permissions and share tokens are for.

## API sketch

Two lanes share the surface. The primary lane is the strict WebSocket
JSON-RPC 2.0 endpoint **`/api/rpc`** (GET = WS upgrade; POST on the same
path and method table = HTTP fallback) — the webui's transport. The
REST lane is a deliberate whitelist for what a JSON control lane cannot
carry (session cookies, binary bodies, probes).

**JSON-RPC methods (`/api/rpc`)** — auth = the httpOnly session cookie
(browsers); `Authorization: Bearer`, `Sec-WebSocket-Protocol: jwt.<t>`
and `?token=` stay open for non-browser clients, tried in that order:

- `auth.verify` — the session's user projection
- `files.list {offset, limit}` · `files.update {id, name}` · `files.delete {id}` (owner-or-admin)
- `users.list {offset, limit}` · `users.create {username, password, display_name?, role?}` · `users.delete {id}` — admin role required
- `settings.get` · `settings.put` — admin role required; `put` takes the COMPLETE strict document (missing sections/leaves → error, never a silent default reset)

Error codes: the JSON-RPC family plus `-32005` auth/permission,
`-32006` not found, `-32007` conflict, `-32008` too large, `-32009`
deferred-ops disabled (this instance serves no `ops.*`).

**REST whitelist**:

- `GET /api/health` — probe / container healthcheck
- `POST /api/v1/auth/login` — password login; plants the `i0721-token` httpOnly cookie (the body carries NO token)
- `POST /api/v1/auth/refresh` — cookie rotation (Set-Cookie needs HTTP, so refresh lives here, not on the RPC table)
- `POST /api/v1/auth/logout` — expires the cookie and supersedes all outstanding tokens
- `POST /api/v1/files` — multipart `file` upload (binary + progress events; optional `?name=`)
- `GET /media/{key}[?thumbnail=true]` — the public path legacy image links depend on (referrer allow-list honored)

## Debug

`just fetch` once to stage the shared recipes, then `just gate` — the
full dual-side lane: webui build → rust (fmt → clippy → test →
`cargo deny`) → vitest → TS-contract staleness → i18n completeness. It
needs no external services (the PostgreSQL lane stays opt-in).

The web UI lives in `packages/webui` (pnpm workspace): `just webui`
builds `dist/webui`, `just webui-test` runs vitest, `just webui-serve`
previews the built dist. The server embeds that dist at compile time
(feature `embedded-frontend`, on by default — build with
`--no-default-features` for a pure-API dev loop; the on-disk dev lane
then reads `I0721_STATIC_DIR`, default `dist/webui`).

The Rust→TS bindings under `packages/webui/src/types` are generated by
ts-rs (`just webui-types`); `just webui-types-check` fails the gate
when they drift. Locale catalogs live under `res/i18n/locales` and are
enforced by `just i18n-check`.

The opt-in PostgreSQL lane (`just pg-test`) runs the `pg_*` store tests
against a scratch database — fail-closed when
`I0721_TEST_PG=1 I0721_DATABASE_URL=…` is set (connection or migration
failures panic instead of skipping).
