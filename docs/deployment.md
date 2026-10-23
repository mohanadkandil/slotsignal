# Deployment

## Fly.io

The current app is `heilbronn-rp-alert-mk-2026` in the personal organization. `fly.toml` runs one 256MB worker in Frankfurt with a 1GB volume. The health endpoint is private; the app has no public service or public IP.

For a new deployment, change the app name in `fly.toml` and run:

```sh
flyctl auth login
flyctl apps create YOUR_APP --org YOUR_ORG
flyctl secrets import --stage --app YOUR_APP < .env
flyctl deploy --app YOUR_APP --local-only --ha=false --no-public-ips
```

Keep one worker. Two processes polling the same Telegram bot will conflict. The `--ha=false` option prevents Fly from creating a spare Machine.

The container first fixes ownership of the data volume, then runs the worker as user 10001. It reads settings from `config.json` and writes state to `/data/status.json`. Credentials come from Fly secrets. The container contains the compiled Rust binary, with no Python runtime.

Check the running app:

```sh
flyctl status --app heilbronn-rp-alert-mk-2026
flyctl checks list --app heilbronn-rp-alert-mk-2026
flyctl logs --app heilbronn-rp-alert-mk-2026
```

The health check fails if the loop or Telegram polling is more than 150 seconds old, a website check has not succeeded in 20 minutes, or three consecutive checks have failed. After the appointment window ends, website freshness is no longer required. Telegram commands remain available.

Fly's restart policy handles process exits. A failed health check reports a problem; it does not restart a running process. The daily message is also sent by this process, so it cannot warn you about a total outage.

To pause the worker:

```sh
flyctl scale count worker=0 --app heilbronn-rp-alert-mk-2026 --yes
```

The volume remains billable while retained. Use Fly's app removal flow when you no longer need the service or its stored state. Check [Fly's current prices](https://docs.fly.io/about/pricing/) for your region.

## Docker Compose

On an existing Linux server, create and pair `.env`, then run:

```sh
docker compose up -d --build
docker compose ps
docker compose logs --tail=50
```

State is saved in the `monitor-data` volume. To stop the worker while retaining that state:

```sh
docker compose down
```

## Before a code update

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

After deployment, send `/test` and `/check` in Telegram. Look for `Runtime: Rust` and a recent successful website check. After at least five minutes, confirm that the successful-check count advances without sending `/check` yourself.
