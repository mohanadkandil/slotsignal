#!/usr/bin/env python3
"""Always-on monitor and private Telegram commands. No incoming ports required."""
import argparse
import datetime as dt
import fcntl
import json
import os
from pathlib import Path
import signal
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, HTTPServer

from telegram_client import call, load_env, send_message

CODE = Path(__file__).resolve().parent
load_env(CODE / ".env")
os.environ.setdefault("ALERT_CHANNELS", "telegram")
os.environ.setdefault("ALERT_DATA_DIR", str(CODE / "data"))
import monitor as m

STOP = False


def status_text(config, state):
    checked = state.get("last_success", "not yet successful")
    result = ["SlotSignal | Heilbronn", f"Server time: {dt.datetime.now(m.TZ).isoformat(timespec='seconds')}",
              f"Window: {config['first_date']} through {config['last_date']} inclusive",
              "Checks: every 5 minutes", f"State: {state.get('status', 'starting')}",
              f"Last successful availability check: {checked}",
              f"Successful checks since setup: {state.get('successful_checks', 0)}"]
    if state.get("earliest_available_date"):
        result.append("Earliest date returned by website: " + state["earliest_available_date"])
    if state.get("status") == "watching":
        result.append("No matching appointments at the last successful check.")
    if state.get("matches") and state.get("status") == "available":
        result.append("Opening: " + json.dumps(state["matches"]))
    if state.get("error"):
        result.append("Attention: " + state["error"])
    if state.get("telegram_error"):
        result.append("Telegram communication: " + state["telegram_error"])
    if state.get("last_notification_at"):
        result.append("Last Telegram delivery accepted: " + state["last_notification_at"])
    result.append("/status = health; /test = example alert; /check = live check")
    return "\n".join(result)


def test_message(config):
    sample = max(dt.date.fromisoformat(config["first_date"]), dt.datetime.now(m.TZ).date())
    m.notify("TEST ONLY — example appointment alert",
             f"Example date: {sample.isoformat()} at 09:00 (Europe/Berlin)."
             "\nThis is simulated, not an available appointment."
             "\nThe booking link below is real. No appointment was booked.")


def authorized(message):
    chat = message.get("chat", {})
    user = message.get("from", {})
    expected = os.environ.get("TELEGRAM_CHAT_ID")
    return (chat.get("type") == "private" and str(chat.get("id")) == expected
            and str(user.get("id")) == expected and not user.get("is_bot", False))


def handle_update(update, config, state, timestamp=None):
    message = update.get("message", {})
    if not authorized(message):
        return
    now = time.time() if timestamp is None else timestamp
    # Ignore old queued commands after downtime; status stays available on request.
    if now - message.get("date", 0) > 600:
        return
    command = message.get("text", "").split(" ", 1)[0].split("@", 1)[0]
    if command == "/test":
        if now - state.get("last_test_at", 0) >= 30:
            test_message(config)
            state["last_test_at"] = now
    elif command == "/check":
        if now - state.get("last_command_check_at", 0) >= 60:
            m.run(config, state, force=True)
            state["last_command_check_at"] = now
            m.write_json(m.ROOT / "status.json", state)
        send_message(status_text(config, state), m.BOOKING_URL)
    elif command in ["/status", "/start", "/help"]:
        send_message(status_text(config, state), m.BOOKING_URL)


def healthcheck():
    state = m.read_json(m.ROOT / "status.json", {})
    if time.time() - state.get("loop_at", 0) > 150:
        return 1
    if state.get("status") == "expired":
        return 0
    polled = state.get("last_telegram_poll")
    if not polled or (dt.datetime.now(m.TZ) - dt.datetime.fromisoformat(polled)).total_seconds() > 150:
        return 1
    success = state.get("last_success")
    if not success:
        return 1
    age = (dt.datetime.now(m.TZ) - dt.datetime.fromisoformat(success)).total_seconds()
    return int(age > 1200 or state.get("consecutive_errors", 0) >= 3)


def start_health_server():
    port = int(os.environ.get("ALERT_HEALTH_PORT", "0"))
    if not port:
        return

    class Handler(BaseHTTPRequestHandler):
        def do_GET(self):
            if self.path != "/health":
                self.send_error(404)
                return
            try:
                healthy = healthcheck() == 0
            except Exception:
                healthy = False
            body = json.dumps({"healthy": healthy}).encode()
            self.send_response(200 if healthy else 503)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, *args):
            pass

    http = HTTPServer(("0.0.0.0", port), Handler)
    threading.Thread(target=http.serve_forever, daemon=True).start()


def serve():
    global STOP
    config = m.read_json(CODE / "config.json")
    if config["interval_seconds"] < 300:
        raise RuntimeError("Minimum check interval is five minutes")
    m.ROOT.mkdir(parents=True, exist_ok=True)
    with (m.ROOT / ".lock").open("w") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        state = m.read_json(m.ROOT / "status.json", {})
        me = call("getMe")
        if call("getWebhookInfo").get("url"):
            raise RuntimeError("This bot already has a webhook; use a dedicated new bot")
        call("setMyCommands", {"commands": [
            {"command": "status", "description": "Last successful check and monitoring health"},
            {"command": "check", "description": "Check availability now"},
            {"command": "test", "description": "Send a clearly marked example alert"},
            {"command": "help", "description": "Monitor help"},
        ]})
        print(f"Telegram worker started: @{me['username']}", flush=True)
        state["started_at"] = dt.datetime.now(m.TZ).isoformat()
        startup_sent = False
        while not STOP:
            m.run(config, state)
            state["loop_at"] = time.time()
            m.write_json(m.ROOT / "status.json", state)
            now = dt.datetime.now(m.TZ)
            try:
                if not startup_sent:
                    send_message("SERVER STARTED\n\n" + status_text(config, state), m.BOOKING_URL)
                    startup_sent = True
                    state["heartbeat_date"] = now.date().isoformat()
                if now.hour >= 9 and state.get("heartbeat_date") != now.date().isoformat():
                    send_message("DAILY HEALTH CHECK\n\n" + status_text(config, state), m.BOOKING_URL)
                    state["heartbeat_date"] = now.date().isoformat()
                updates = call("getUpdates", {
                    "offset": state.get("telegram_offset", 0), "timeout": 20,
                    "allowed_updates": ["message"],
                })
                for update in updates:
                    handle_update(update, config, state)
                    state["telegram_offset"] = update["update_id"] + 1
                state.pop("telegram_error", None)
                state["last_telegram_poll"] = dt.datetime.now(m.TZ).isoformat()
            except Exception as error:
                state["telegram_error"] = str(error)
                print("Telegram communication failed; retrying in 20 seconds.", flush=True)
                for _ in range(20):
                    if STOP:
                        break
                    time.sleep(1)
            state["loop_at"] = time.time()
            m.write_json(m.ROOT / "status.json", state)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--healthcheck", action="store_true")
    parser.add_argument("--test", action="store_true")
    args = parser.parse_args()
    if args.healthcheck:
        sys.exit(healthcheck())
    if args.test:
        test_message(m.read_json(CODE / "config.json"))
        print("Telegram accepted the test message. Confirm that you received it.")
        return

    def stop(signum, frame):
        global STOP
        STOP = True

    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGINT, stop)
    start_health_server()
    serve()


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        print(f"Server stopped: {type(error).__name__}: {error}", file=sys.stderr)
        sys.exit(1)
