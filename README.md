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
- Internationalization
- Deploy via Docker Compose
- Written in pure Rust

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

The service listens on `127.0.0.1:8080` (override with
`I0721_PUBLISH=127.0.0.1:9090`). `GET /api/health` reports status and
where the site configuration came from.

### Single container

The image also runs standalone against an external PostgreSQL:

```bash
docker run -d -p 8080:80 \
  -e I0721_DATABASE_URL='postgres://i0721:secret@db-host:5432/i0721' \
  -e JWT_SECRET='a-long-random-string' \
  -v i0721-media:/app/data \
  ghcr.io/langyo/0721:latest
```

Migrations apply automatically at boot (bookkeeping is compatible with
the legacy `seaql_migrations` table). On an empty user table the service
seeds an `admin`/`admin` account — change the password after the first
login (create a second admin, then delete or re-role the default one).

## Configuration

Process configuration is environment-only (fail-fast — an unset
required variable or an unparseable value refuses to start):

| Variable | Default | Meaning |
|---|---|---|
| `I0721_DATABASE_URL` | required | PostgreSQL URL |
| `JWT_SECRET` | required | HS256 secret for the transition-era tokens |
| `I0721_LISTEN` | `0.0.0.0:80` | HTTP bind address |
| `I0721_MEDIA_ROOT` | `data` | blob root (`media/` and `media-cache/` below it) |
| `I0721_STATIC_DIR` | `res/website` | placeholder static site directory |
| `I0721_UPLOAD_CONCURRENCY` | `4` | concurrent upload-pipeline slots |
| `I0721_LOG` | `i0721_server=info,tower_http=warn` | tracing filter |

The site configuration (the old `Config.toml`) lives in the `settings`
table and is edited through the admin API (`GET/PUT /api/v1/settings`) —
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

## API sketch

- `GET /api/health`
- `POST /api/v1/auth/login` · `GET /api/v1/auth/verify` · `POST /api/v1/auth/refresh` — password + JWT (15-day tokens, 5 logins/min/IP)
- `GET /api/v1/files` · `POST /api/v1/files` (multipart `file`, optional `?name=`) · `PATCH /api/v1/files/{id}` · `DELETE /api/v1/files/{id}` (soft delete)
- `GET /api/v1/users` · `POST /api/v1/users` · `DELETE /api/v1/users/{id}` — admin role required
- `GET /api/v1/settings` · `PUT /api/v1/settings` — admin role required
- `GET /media/{key}[?thumbnail=true]` — the public path legacy image links depend on (referrer allow-list honored)

## Debug

`just fetch` once to stage the shared recipes, then `just gate`
(fmt → clippy → test → `cargo deny`). This is the same lane the CI
dispatch workflow runs; it needs no external services.

The opt-in PostgreSQL lane (`just pg-test`) runs the `pg_*` store tests
against a scratch database — fail-closed when
`I0721_TEST_PG=1 I0721_DATABASE_URL=…` is set (connection or migration
failures panic instead of skipping). The hikari webui wave will bring
the new frontend dev lane.
