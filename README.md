# SlotSignal

A small, always-on appointment watcher that sends Telegram alerts with the date, available times, and a direct booking button.

Configured for **Heilbronn residence permit renewal**, one person, **23 October–14 November 2026 inclusive**. It checks every five minutes. You make the booking yourself.

## Use it

Open [SlotSignal on Telegram](https://t.me/HeilbronnSlotsMK2026Bot).

| Command | What it proves |
| --- | --- |
| `/status` | The server is responding; shows its time, last successful website check, successful check count, and any errors. |
| `/check` | Makes a real availability check now and returns the result. Limited to once per minute. |
| `/test` | Sends a **TEST ONLY** example using the real alert delivery path and booking link. No real opening is implied. |
| `/help` | Shows the monitor status and command guide. |

You receive a daily health message after **09:00 Europe/Berlin**. A real opening alert names the date and times in the same timezone and includes an **Open renewal booking** button.

## How reliable is it?

- **Independent of your laptop:** one always-on Fly.io worker in Frankfurt, with persistent state on a 1GB volume. The worker runs as an unprivileged user after initializing the volume.
- **Checks the actual appointment feed:** uses the same read-only availability endpoint as the booking page. If a date is in range, it confirms that day's time slots before alerting.
- **Detects failures:** connection timeouts, unexpected responses, and disabled or changed services are reported as errors. Three consecutive failures trigger an attention alert, with retries and backoff.
- **Avoids repeated messages:** alerts immediately for a new available day, then reminds after 30 minutes if it is still available. State survives normal restarts and deployments.
- **Reports health:** Fly checks a private `/health` endpoint. It becomes unhealthy when the loop stalls, Telegram polling is stale, or website checks fail for too long. Fly's restart policy handles process exits; a failed health check alone does not restart a live process.
- **Restricts access:** only the paired private Telegram chat can run commands. Credentials live in Fly secrets and a local owner-readable `.env`, outside Git and Docker build layers.

**Limits:** five-minute polling can miss a slot that opens and disappears between checks. An alert cannot hold a slot or guarantee a booking. Telegram accepting a message does not guarantee a phone notification; unmute the bot and enable Telegram notifications. A total server, host, or network outage cannot report itself; a missing daily message or unanswered `/status` needs investigation. This is a single worker, not a highly available service.

Appointment requests stop after 14 November. The bot can still respond to `/status`; Fly resources keep billing until stopped or removed. See [Fly pricing](https://docs.fly.io/about/pricing/) for current rates.

## How to test it yourself

1. Send `/test`. Verify the clearly marked example and its booking button on your phone and laptop.
2. Send `/check`. Verify a recent successful website check, or a clearly reported error.
3. Send `/status` again after at least five minutes. The successful-check count and timestamp should advance automatically.
4. Put your laptop to sleep. Send `/status` from your phone. A fresh server time demonstrates that Fly, rather than your laptop, is running the watcher.
5. Expect the daily health message the next morning.

Automated tests use simulated openings and mocked Telegram responses. They cover date filtering, disappeared slots, duplicates, delivery retries, expiry, malformed responses, link buttons, credential redaction, command authorization, and health reporting. They do not replace the live Telegram and website checks above.

## Development

Python 3.9+; standard library only. The production container uses Python 3.12.

```sh
python3 -m unittest -v test_alerts
bash -n setup-telegram.command
```

Edit `config.json` to change the date range, interval, or reminder period. Intervals shorter than five minutes are rejected by deployment setup.

The optional `setup-telegram.command` helper creates and pairs a dedicated bot using official BotFather. It uses hidden token input, a random chat-pairing challenge, and a test message. Never paste bot credentials into issues or commit `.env`.

## Deploy to Fly.io

`fly.toml` defines one 256MB shared CPU worker, a persistent volume, a private health check, and no public service or public IP. Set the app name for a separate deployment.

```sh
flyctl auth login
flyctl apps create YOUR_APP --org YOUR_ORG
flyctl secrets import --stage --app YOUR_APP < .env
flyctl deploy --app YOUR_APP --local-only --ha=false --no-public-ips
flyctl status --app YOUR_APP
flyctl checks list --app YOUR_APP
flyctl logs --app YOUR_APP
```

Keep **one** worker: multiple instances polling the same Telegram bot conflict. `--ha=false` avoids creating a spare Machine. To pause the deployed service:

```sh
flyctl scale count worker=0 --app YOUR_APP --yes
```

The volume remains billable while retained. To permanently remove the app and its stored state, use the Fly dashboard's app removal flow.

## Other runtimes

`docker compose up -d --build` runs the same worker on an existing Linux host; `.env` must contain `TELEGRAM_BOT_TOKEN` and `TELEGRAM_CHAT_ID`. State lives in the named `monitor-data` volume.

`manage.py` manages the original Mac-only fallback: `install`, `status`, `check`, `test-notification`, and `stop`. It needs an awake, online Mac. Its runtime is `~/Library/Application Support/HeilbronnAppointmentAlert`; it does not share the server's Telegram polling session.

## Source of appointments

[Official booking page](https://www.etermin.net/qtermin-stadtheilbronn-abh?lang=en), service **144512**, “extension of a residence permit.” Website settings and service details are refreshed hourly so the watcher follows the site's lead times and booking constraints. It does not bypass access controls or reserve appointments.

If your permit expires soon, do not treat an appointment reservation or this monitor as a renewal application. Follow [Heilbronn's official guidance](https://www.heilbronn.de/rathaus/buergerservice-a-z/inhalt/faqs-auslaenderangelegenheiten.html); contact **abh@heilbronn.de** or **07131 56-2064**.
