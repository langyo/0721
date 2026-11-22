<div align="center">
  <img src="https://raw.githubusercontent.com/celestia-island/celestia-island.github.io/main/res/logos/0721.webp" alt="0721" width="180" />
</div>

<h1 align="center">0721</h1>

<div align="center">
  <strong>A simple engine for image hosting, netdisk and galleries, written in Rust.</strong>
</div>

<div align="center">
  Ciallo～(∠・ω< )⌒★
</div>

<br />

<div align="center">
  <a href="https://github.com/langyo/0721/actions/workflows/ci.yml">
    <img src="https://img.shields.io/github/actions/workflow/status/langyo/0721/ci.yml?label=CI" alt="CI" />
  </a>
  <a href="https://github.com/langyo/0721/blob/master/LICENSE">
    <img src="https://img.shields.io/github/license/langyo/0721" alt="License: Apache-2.0" />
  </a>
  <a href="https://github.com/langyo/0721/tags">
    <img src="https://img.shields.io/github/v/tag/langyo/0721" alt="Tag" />
  </a>
</div>

Rebuilt on axum, lacrymira and hikari; OAuth sign-in, S3/IPFS storage
backends and an MCP surface for AI agents.

## Features

- Image hosting
- Automatic image compression via WebP (animated GIF included)
- Netdisk-ready metadata schema (kinds, directories, soft delete, shares)
- Basic account system (admin-gated user management)
- OAuth dual-mode login: provider registry (GitHub / Feishu / ERP
  templates + generic OIDC discovery), PKCE S256 authorization-code
  flow, optional password login with a kill-switch
- Site configuration stored in the database — no restart to apply
- hikari web UI (Vue 3 + TypeScript): portal timeline with upload
  progress, user roster, strict site settings, theme (light/dark/auto +
  palettes) — embedded into the release binary
- Internationalization (JSON catalogs, 12 locales, en + zh-Hans baselines)
- Deploy via Docker Compose
- Written in Rust with a TypeScript frontend

## Storage backends

The blob backend is selected at startup by `I0721_STORAGE_BACKEND`
(`local` | `s3` | `ipfs`; default `local`). All three implement the same
put/get/delete surface; thumbnails (the Cache slot) follow the same
backend. Multi-backend coexistence and per-file overrides are a future
wave — this is a deployment-shape choice.

| Backend | Env | Notes |
|---|---|---|
| `local` (default) | `I0721_MEDIA_ROOT` (default `data`) | Two directories: `media/` + `media-cache/` |
| `s3` | `I0721_S3_ENDPOINT` `I0721_S3_BUCKET` `I0721_S3_REGION` `I0721_S3_ACCESS_KEY` `I0721_S3_SECRET_KEY` | Works with AWS S3, MinIO, R2. Endpoint present = path-style; absent = AWS. Keys: `media/{sha}` and `cache/{sha}.webp` |
| `ipfs` | `I0721_IPFS_API` (e.g. `http://127.0.0.1:5001`) | kubo RPC. put = `/api/v0/add?pin=true` + MFS link; get = MFS read; delete = MFS rm + best-effort unpin |

Semantic differences:
- **S3** has no local cache — every read is a network round-trip. For
  single-node deployments with a local MinIO this is still fast.
- **IPFS** deletion is content-addressed: `delete` unpins the CID, but
  another row referencing the same content keeps it alive; kubo's
  garbage collector reclaims unpinned CIDs on its schedule. The
  key-to-CID mapping rides kubo's MFS (`/i0721/{slot}/{key}`) —
  node-local state; a multi-node cluster would need the mapping in the
  metadata database instead.

**Backend switching**: changing `I0721_STORAGE_BACKEND` affects NEW
uploads only, and reads always go through the single configured
backend — the `files.storage_backend` column is per-row bookkeeping,
never consulted on read. After a switch, blobs written to the old
backend become unreachable until you switch back or migrate them —
their media links answer 404 while the old backend is still reachable
but the blob is gone, and 500 when the old backend itself is down;
a blob-migration tool arrives with a later wave.

**Security note**: `I0721_S3_ENDPOINT` with `http://` sends credentials
in cleartext — this is safe ONLY for internal-network MinIO (the
compose example). Production deployments must use `https://`.

`I0721_MEDIA_ROOT` has no effect when the backend is `s3` or `ipfs`
(it only configures the `local` backend's directory layout).

Deployment examples: `deploy/docker-compose.s3.yml` (service + MinIO +
create-bucket init) and `deploy/docker-compose.ipfs.yml` (service +
kubo).

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
| `I0721_PUBLIC_ORIGIN` | — | the deployment's absolute https origin (OAuth redirect URIs); unset = Host-header derivation |
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

## Login (dual mode)

The service offers two login modes, driven by configuration:

1. **OAuth providers** (the open-source default): the admin registers
   providers on the webui's "Login providers" page (or the
   `providers.*` RPC) — a built-in endpoint template (`builtin:github`,
   `builtin:feishu`, `builtin:erp` with its `base_url`) or a generic
   OIDC issuer (endpoints via `/.well-known/openid-configuration`,
   cached 10 min). Each provider row carries its `client_secret`
   server-side; every public shape shows only `has_secret` + the last
   four characters. The browser flow is a standard authorization-code
   round trip with **PKCE S256** and a 600 s HttpOnly state cookie; a
   first login mints a local account (`{provider}_{subject[0..8]}`,
   no password) bound through the `oauth_identities` table — the same
   (provider, subject) always signs into the same account.
2. **Password login** (optional): on unless turned off. The switch
   lives in the admin settings ("Allow password login") or the
   settings row `auth_password_login`; the env pin
   `I0721_PASSWORD_LOGIN=true|false` sits between the row and the
   default (row > env > on). **Turn it off once OAuth is configured** —
   a fresh deployment stays on so it cannot lock itself out.

Security notes: the session credential is an httpOnly cookie (never in
page-readable storage or URLs); the OAuth `client_secret` never leaves
the server; the state cookie is strictly compared against the
provider's echo AND carries the provider id (a state minted for one
provider cannot be replayed against another); provider endpoint URLs
must be absolute https (http survives only for localhost/127.0.0.1 —
the local dev/stub lane); set `I0721_PUBLIC_ORIGIN`
(https://your-domain.example in production) so the OAuth redirect URI
follows the deployment's real origin instead of the Host header; the
password kill-switch refuses to turn the password OFF while no OAuth
provider is enabled (the no-login-method state is unreachable); no
email-domain restriction ships in this first version (the admin gate +
provider choice are the boundary).

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

**Transport**: the webui prefers the WebSocket (heartbeat + latency);
the HTTP POST fallback on the same method map rides the session cookie.
The WS upgrade carries a ONE-TIME admission ticket in its `?token=`
slot: minted by `POST /api/v1/auth/ws-ticket` (≤30 s, user-bound),
consumed by the opening itself, worth exactly one socket — a leaked
ticket has bounded value, and the session JWT never rides a URL.
(Declared deviation: the credential extractor exists twice —
`api/auth.rs` for REST and `rpc.rs` for the WS/POST lanes — an
intentional dual-write pending a shared auth module; both keep the
same four-channel order.)

Error codes: the JSON-RPC family plus `-32005` auth/permission,
`-32006` not found, `-32007` conflict, `-32008` too large, `-32009`
deferred-ops disabled (this instance serves no `ops.*`).

**REST whitelist**:

- `GET /api/health` — probe / container healthcheck
- `POST /api/v1/auth/login` — password login; plants the `i0721-token` httpOnly cookie (the body carries NO token)
- `POST /api/v1/auth/refresh` — cookie rotation (Set-Cookie needs HTTP, so refresh lives here, not on the RPC table)
- `POST /api/v1/auth/logout` — expires the cookie and supersedes all outstanding tokens
- `POST /api/v1/auth/ws-ticket` — mints the one-time WebSocket admission ticket
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
