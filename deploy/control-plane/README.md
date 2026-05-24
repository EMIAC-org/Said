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

## GitHub Actions Secrets

Set these repository secrets for CI/CD:

- `DEPLOY_HOST`: server IP or hostname
- `DEPLOY_USER`: SSH user
- `DEPLOY_SSH_KEY`: private key with access to the server
- `DEPLOY_PATH`: usually `/opt/airnote-control-plane`

The deploy workflow builds and pushes a GHCR image, copies the compose files to the server, writes the new image tag into `.env`, and runs:

```bash
docker compose pull control-plane
docker compose up -d
docker image prune -f
```

Database schema migrations run automatically on `control-plane` startup.
