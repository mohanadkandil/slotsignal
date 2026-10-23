# SlotSignal

SlotSignal checks Heilbronn's residence permit renewal calendar and sends you a Telegram message when an appointment fits your dates. The message includes the date, available times, and a booking button.

It runs as one Rust process on Fly.io. Your laptop can be off.

The current search is for one person, from today through **14 November 2026**, with a check every five minutes. All times are in Europe/Berlin.

## Try it

Open [SlotSignal in Telegram](https://t.me/HeilbronnSlotsMK2026Bot).

- Send `/test` for an example alert. It says `TEST ONLY`; the example date is made up.
- Send `/check` to check the actual appointment website now.
- Send `/status` to see the server time, last successful check, and any errors.

After five minutes, the check count and timestamp should advance by themselves. To check that the server works without your laptop, put the laptop to sleep and send `/status` from your phone.

## What it checks

Every five minutes, the process asks the booking system for dates for "extension of a residence permit" (service 144512). If a date falls in your search window, it asks for that day's available times before sending an alert.

You still need to open the link and book. SlotSignal cannot hold an appointment for you. It sends another reminder after 30 minutes if the same day is still available.

## What can go wrong

A slot can open and disappear between checks. Someone can also take it before you click the alert. Five-minute polling cannot prevent either case.

The worker retries failed requests and reports errors separately from an empty calendar. After three failed checks or alert deliveries, it tries to send an attention message. Repeated failures slow the checks to at most one per hour.

Fly restarts the process if it exits. Status and alert history are saved on a volume, so a normal restart does not erase them. A health check watches the loop, recent Telegram replies, and successful website checks. A failed health check alone does not restart the process.

You get a daily health message after 09:00. If the whole server or its network goes down, it cannot send that message. An unanswered `/status` or a missing daily message is a reason to investigate. There is only one worker, so a host failure can interrupt monitoring.

Unmute the bot and allow Telegram notifications on your phone and laptop. A message reaching Telegram does not prove that your phone showed a notification.

Appointment checks stop after 14 November. The bot can still answer commands, and Fly resources keep billing until you stop or remove them.

## Run it locally

Use Rust 1.97.1 or later.

```sh
cargo test --locked
cargo build --release --locked
cp .env.example .env
chmod 600 .env
```

Create a dedicated bot with [BotFather](https://t.me/BotFather) using `/newbot`. Put its token in `.env`, then pair your private chat:

```sh
./target/release/slotsignal pair
./target/release/slotsignal test
./target/release/slotsignal serve
```

The pairing command prints a link. Open it in Telegram and press Start. It saves your chat ID in `.env`. Keep that file out of Git; the repository already ignores it. Only your paired private chat can run bot commands.

Edit `config.json` to change the dates or reminder period. Checks and reminders must be at least five minutes apart.

## Deploy and maintain it

[Deployment instructions](docs/deployment.md) cover Fly.io, Docker Compose, logs, and stopping the worker. Credentials go into Fly secrets, not the image or repository.

The tests cover date filtering, slots disappearing before confirmation, duplicate alerts, delivery retries, saved state, private-chat access, booking links, and errors that could expose credentials. Live checks are still needed to confirm that the website and your Telegram chat are reachable.

The appointment source is the [official booking page](https://www.etermin.net/qtermin-stadtheilbronn-abh?lang=en). The worker refreshes service details hourly and follows the website's booking constraints.

If your permit expires soon, follow [Heilbronn's application guidance](https://www.heilbronn.de/rathaus/buergerservice-a-z/inhalt/faqs-auslaenderangelegenheiten.html). An appointment reservation is separate from a renewal application.
