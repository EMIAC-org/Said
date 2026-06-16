# AirNote Control Plane Deploy

This deploys the server-side stack only:

- `control-plane`: Axum API, WebSocket meeting hub, and embedded React admin panel at `/admin`
- `postgres`: production database
- optional `caddy`: HTTPS reverse proxy

## First Server Setup

Install Docker and the Compose plugin on the server, then create the app directory:

```bash
sudo mkdir -p /opt/airnote-control-plane
sudo chown "$USER":"$USER" /opt/airnote-control-plane
```

Copy these files to `/opt/airnote-control-plane`:

- `docker-compose.yml`
- `Caddyfile`
- `.env.example` as `.env`

Edit `.env` and set strong values for:

- `CONTROL_PLANE_IMAGE`
- `POSTGRES_PASSWORD` using URL-safe characters, for example alphanumeric plus `_` or `-`
- `POSTGRES_VOLUME_NAME` when migrating an existing deployment volume
- `JWT_SECRET`
- `LARK_APP_ID`
- `LARK_APP_SECRET`
- `LARK_REDIRECT_URI`
- `OPENAI_CLIENT_ID` (default Codex public client: `app_EMoamEEZ73f0CkXaXp7hrann`)
- `OPENAI_REDIRECT_URI` (use `http://localhost:1455/auth/callback` for the current manual paste flow)

Start without public HTTPS:

```bash
docker compose pull
docker compose up -d
curl http://127.0.0.1:3100/v1/health
```

Start with built-in Caddy HTTPS:

```bash
docker compose --profile caddy up -d
```

## Static Release Hosting

Caddy also serves local release artifacts for the desktop updater:

- `https://airnote.emiactech.com/updates/latest.json`
- `https://airnote.emiactech.com/releases/<version>/...`

The release files live under `${RELEASES_PATH:-./releases}` on the server. The
recommended path is `/opt/airnote-control-plane/releases` when running compose
from `/opt/airnote-control-plane`.

Use the repo helper from a Mac after the app is signed/notarized:

```bash
scripts/deploy-release-vm.sh
```

The script uploads the DMG for manual download plus the signed Tauri updater
bundle (`.app.tar.gz` + `.sig`) and keeps only the latest three versions.
It uses `ssh`/`scp`; set `REMOTE`, `REMOTE_RELEASE_ROOT`, or `PUBLIC_BASE_URL`
to override defaults. If `SSHPASS` is present and `sshpass` is installed, the
script can run non-interactively, but the password should never be committed.

## GitHub Actions Secrets

Set these repository secrets for CI/CD:

- `DEPLOY_HOST`: server IP or hostname
- `DEPLOY_USER`: SSH user
- `DEPLOY_SSH_KEY`: private key with access to the server
- `DEPLOY_PATH`: usually `/opt/airnote-control-plane`
- `LARK_APP_ID`: production Lark app ID
- `LARK_APP_SECRET`: production Lark app secret
- `LARK_REDIRECT_URI`: production callback URL, usually `https://airnote.emiactech.com/v1/auth/lark/callback`

The deploy workflow builds and pushes a GHCR image, copies the compose files to the server, writes the new image tag and Lark OAuth secrets into `.env`, validates those values reached the running container, and runs:

```bash
docker compose pull control-plane
docker compose up -d --force-recreate control-plane
docker image prune -f
```

Database schema migrations run automatically on `control-plane` startup.

## Dev Environment

The `dev` branch deploys with `.github/workflows/deploy-control-plane-dev.yml`.
It reuses the same SSH deploy secrets but keeps a separate compose project,
directory, database, and local ports:

- Deploy path: `/opt/said-control-plane-dev`
- Compose project: `airnote-control-plane-dev`
- API bind: `127.0.0.1:3101`
- Postgres bind: `127.0.0.1:5433`
- Database: `said_control_plane_dev`
- Volume: `airnote-control-plane-dev_postgres-data`

The workflow updates only non-secret dev environment fields and the image tag.
Keep the real dev `.env` on the VM; do not add DB/JWT/OAuth secrets to this repo.
