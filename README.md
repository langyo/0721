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
- MCP surface for AI agents: API-key authenticated SSE transport, five
  netdisk tools, per-key permissions (read / upload / share) with an
  optional expiry, copy-once key reveal
- Deploy via Docker Compose
- Written in Rust with a TypeScript frontend

## Storage backends

The blob backend is selected at startup by `I0721_STORAGE_BACKEND`
(`local` | `s3` | `ipfs`) — required for the first start that runs the
bucket migration, and ignored afterwards. The migration registers one
bootstrap bucket for the chosen backend and seats every file row in it;
from then on bytes are routed by each row's own bucket. All backends
implement the same put/get/delete surface; thumbnails (the Cache slot)
follow the same backend as their row's bucket. Per-bucket registration
and credential management arrive with a later wave; the registry and
management RPCs (`storage.buckets.*`, `storage.grants.set`,
`storage.defaults.*`) are live.

| Backend | Env | Notes |
| --- | --- | --- |
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

**Backend switching**: there is none. Every read routes through the
bucket each file row carries — changing `I0721_STORAGE_BACKEND` no
longer switches backends, because the variable is only consulted for
the first start that runs the bucket migration and is ignored
afterwards. Each row keeps its own bucket forever; rows whose
`storage_backend` label differs from the current backend sit in a
DISABLED legacy bootstrap bucket, and `/api/health` reports that
bucket's state (and the reason it cannot serve) instead of pretending
the data is lost. To serve such rows, register that bucket's
credentials in the bucket management surface.

**Security note**: `I0721_S3_ENDPOINT` with `http://` sends credentials
in cleartext — this is safe ONLY for internal-network MinIO (the
compose example). Production deployments must use `https://`.

`I0721_MEDIA_ROOT` has no effect when the backend is `s3` or `ipfs`
(it only configures the `local` backend's directory layout).

## Buckets, anonymous direct links and shares

Each file row carries its own bucket, and every byte read (original,
thumbnail, share download) routes through THAT bucket — two rows with
the same content hash can sit in different buckets, and `/media/{key}`
resolves per request through the row that authorizes the caller. That
same-key cross-bucket visibility is the premise to remember: content is
keyed by its hash, so a public and a private bucket can both hold the
same key, and the ANSWER depends on which bucket the requesting
identity is allowed to read.

- **Anonymous direct links**: a bucket with `anonymous_read = TRUE`
  serves its media to anyone — no session, no key. A private bucket's
  media answers 404 to anonymous requests (indistinguishable from a
  missing key: no 403, no 503 oracle), while a signed-in session or API
  key the bucket grants `Read` still gets 200 with a `private, no-store`
  cache directive.
- **Cache non-revocation**: `public, max-age=31536000, immutable` is
  issued whenever the serving bucket is anonymous-readable. Flipping
  `anonymous_read` to false CANNOT retract caches already issued to
  browsers and intermediaries — the directive is the point of the
  feature. Emergency takedown means deleting the OBJECT (or the file
  row), never toggling the flag.
- **`s3:*` scope**: each registered bucket's credentials are the
  deployment-wide S3 identity scoped to that bucket's ARN (with
  ListBucket). The bucket system trusts the storage credential's
  permission face; per-user authorization lives entirely in the
  `bucket_grants` table, and a mis-scoped credential would let the
  service read/write outside what the grants expect — keep the
  credential policies `s3:*` on the bucket's own ARN only.
- **Shares survive privatisation**: revoking a bucket's
  `anonymous_read`, disabling it, or revoking the owner's access does
  NOT invalidate shares already issued — a share's bytes route through
  the file ROW's bucket, and the owner's `files.share_list` keeps
  listing them. Creating a NEW share still requires the actor's current
  share bit on the bucket at call time.

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
healthchecks (`pg_isready` + /api/health with start periods).

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
| --- | --- | --- |
| `I0721_DATABASE_URL` | required | PostgreSQL URL |
| `JWT_SECRET` (container-side; compose sets it from `I0721_JWT_SECRET`) | required | HS256 secret for the transition-era tokens |
| `I0721_LISTEN` | `0.0.0.0:8080` | HTTP bind address (keep the port unprivileged: the image runs as a non-root user) |
| `I0721_STORAGE_BACKEND` | required once | blob backend selection: `local`, `s3` or `ipfs` — required for the first start that runs the bucket migration; ignored afterwards (see [Storage backends](#storage-backends)) |
| `I0721_MEDIA_ROOT` | `data` | blob root (`media/` and `media-cache/` below it); consumed by the `local` backend only |
| `I0721_S3_ENDPOINT` | *(empty)* | S3-compatible endpoint (MinIO/R2; http only for internal networks). Empty = AWS |
| `I0721_S3_BUCKET` / `I0721_S3_REGION` | required with `s3` | bucket name / region (`auto` works for MinIO) |
| `I0721_S3_ACCESS_KEY` / `I0721_S3_SECRET_KEY` | required with `s3` | credentials |
| `I0721_IPFS_API` | required with `ipfs` | kubo RPC base URL (e.g. `http://127.0.0.1:5001`) |
| `I0721_PUBLIC_ORIGIN` | — | the deployment's absolute https origin (OAuth redirect URIs, MCP share links); unset = Host-header derivation |
| `I0721_STATIC_DIR` | `dist/webui` | web UI dist directory (dev mode; the default build embeds the dist) |
| `I0721_UPLOAD_CONCURRENCY` | `4` | concurrent upload-pipeline slots |
| `I0721_LOG` | `i0721_server=info,tower_http=warn` | tracing filter |
| `I0721_SEED_ADMIN_PASSWORD` | `admin` | first-boot admin password (empty user table only) |
| `I0721_COOKIE_SECURE` | `auto` | session cookie `Secure` flag: `auto` (follow `X-Forwarded-Proto`), `always` (TLS edge that does not forward the scheme) or `never`; anything else refuses to start |
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
1. **Password login** (optional): on unless turned off. The switch
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
- `keys.create {name, ops, expires_at?}` · `keys.update {id, name, ops, expires_at?}` · `keys.list` · `keys.revoke {id}` — the MCP API keys; `create` answers the plaintext key exactly once, `update` replaces the whole record (name, permissions, expiry — an omitted `expires_at` clears it), `revoke` also kills the live MCP sessions that key opened
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
- `GET /media/{key}[?thumbnail=true]` — the public path legacy image links depend on (referrer allow-list honored). Access is judged by the file row's BUCKET: a bucket with `anonymous_read = TRUE` serves anyone (200 or 404 — an anonymous request never distinguishes availability); otherwise a signed-in caller whose bucket grants `Read` still gets 200, a visible-but-unreadable bucket answers 403, and an unusable bucket answers 503. Cache directives follow the same predicate: anonymous-readable 2xx responses carry `public, max-age=31536000, immutable`, everything else is `no-store` (private 2xx) or `no-store` (all failures)
- `GET /api/mcp/sse` — the MCP stream handshake, authenticated by an `Authorization: Bearer i0721-…` API key (minted on the API keys page or via `keys.create`). The session cookie is deliberately NOT accepted here: an agent carries the key, not a browser session, and a cookie-authenticated stream would let any page a signed-in browser visits open one
- `POST /api/mcp/message?sessionId=…` — one JSON-RPC frame for that stream; the session id comes from the stream's first `endpoint` frame, and the response is pushed back over the stream, so the POST itself only answers 202

## MCP

The MCP surface lets an AI agent drive the netdisk: five tools over the
SSE transport, authenticated with an API key instead of a browser
session.

| Lane | Shape |
| --- | --- |
| `GET /api/mcp/sse` | the stream; `Authorization: Bearer i0721-…`. The first frame is `event: endpoint`, carrying `/api/mcp/message?sessionId=…` |
| `POST /api/mcp/message?sessionId=…` | one JSON-RPC frame; answers 202 and pushes the response back through the stream |

JSON-RPC 2.0 conventions on this surface: protocol failures (`-32600`
invalid request, `-32601` unknown method, `-32602` unknown tool /
invalid params, `-32001` permission denied, `-32700` parse error) answer the
envelope's top-level `error` object, while a tool that RUNS and fails is a
`result` with `isError: true`; a request without `id` is a notification
(`notifications/initialized`) — 202 and no frame; a body the server
cannot read at all answers an HTTP 400 whose body carries the prescribed
`id: null` error frame. A session lives 30 minutes, dies with its
stream, and is dropped the moment its key is revoked or expires.

| Tool | Arguments (`*` required) | Result |
| --- | --- | --- |
| `list_files` | `folder_id`, `offset` (default 0), `limit` (default and cap 100) | one folder of the caller's tree, newest first |
| `file_info` | `id`* | one file's metadata |
| `upload_file` | `content_base64`*, `name`*, `folder_id` | the created file — the SAME chain as the REST upload lane (quota, folder ownership, WebP pipeline, configured storage backend) |
| `get_share_link` | `file_id`*, `expires_at`, `allow_preview` (default true) | `{url, token}`; a live share is reused, an expired one is replaced, a folder is refused |
| `search_files` | `query`*, `kind`, `offset` (default 0), `limit` (default and cap 500) | files whose name (or original name) matches, scanned page by page |

`tools/list` carries a real JSON Schema `inputSchema` per tool, so a
client discovers the arguments without guessing — and it lists only the
tools the presenting key's permissions cover. The permissions are per
key and enforced per tool: `read` covers `list_files` / `file_info` /
`search_files`, `upload` covers `upload_file`, `share` covers
`get_share_link`. Calling a tool outside the key's set answers the
JSON-RPC `-32001` error frame (`scope denied: this API key lacks the
"upload" permission required by tool "upload_file"`) without running
the tool. Every call runs with the key owner as the acting user — an
API key never elevates. The share
URL's origin is `I0721_PUBLIC_ORIGIN` (else the request's `Host`); with
neither configured `get_share_link` answers an explicit error instead of
minting a `localhost` link nobody can open.

**Keys**: create one on the webui's "API keys" page (or `keys.create` on
`/api/rpc`) — the plaintext key is shown exactly ONCE; `keys.list` shows
only the masked prefix, and `keys.revoke {id}` kills the key and every
live session it opened.

Every key carries its OWN permissions and an optional expiry, set at
creation or with `keys.update {id, name, ops, expires_at?}` (a
whole-record replacement; omit `expires_at` to clear it — omitted,
`null` or `""` all clear it and never mean "keep the old one"). A key
needs at least one permission, and a supplied `expires_at` must be RFC
3339 and in the future; any UTC offset is accepted and the instant is
stored in its canonical UTC form. Expiry is checked at every handshake AND on every frame: an
expired key cannot open a stream, and a session whose key lapses
mid-flight is dropped with a 401. Keys minted before permissions were
enforced are normalized by the schema migration to the EMPTY permission
set — grant them with `keys.update`.

### Upgrading

API keys created before per-key permissions existed start with NO
permissions after the upgrade: their stored permission set was never
enforced, so the migration normalizes it to the empty set instead of
guessing a grant. Such a key is listed on the "API keys" page with a "no
permissions" tag and cannot call any MCP tool until you edit it there and
grant what it needs. Review every key once after deploying this release —
an agent holding an ungranted key sees `tools/list` answer an empty array
and `-32001` on every call.

A copy-pasteable `mcpServers` entry for an SSE-speaking client (Claude
Code, Cursor and friends) — replace the origin and the key:

```json
{
  "mcpServers": {
    "0721": {
      "type": "sse",
      "url": "https://res.example.com/api/mcp/sse",
      "headers": { "Authorization": "Bearer i0721-<your-api-key>" }
    }
  }
}
```

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
