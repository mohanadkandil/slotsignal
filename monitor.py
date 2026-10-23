#!/usr/bin/python3
"""Read-only eTermin availability monitor. Never reserves or books appointments."""
import argparse
import datetime as dt
import fcntl
import json
import os
import subprocess
import sys
import time
import urllib.parse
import urllib.request
from pathlib import Path
from zoneinfo import ZoneInfo

from telegram_client import send_message

CODE = Path(__file__).resolve().parent
ROOT = Path(os.environ.get("ALERT_DATA_DIR", str(CODE))).resolve()
WEBID = "qtermin-stadtheilbronn-abh"
SERVICE = 144512
BOOKING_URL = f"https://www.etermin.net/{WEBID}/serviceid/{SERVICE}?lang=en"
TZ = ZoneInfo("Europe/Berlin")


def read_json(path, default=None):
    try:
        return json.loads(path.read_text())
    except FileNotFoundError:
        return default


def write_json(path, data):
    temporary = path.with_suffix(".tmp")
    temporary.write_text(json.dumps(data, indent=2) + "\n")
    temporary.replace(path)


def api(path, params=None):
    url = "https://www.etermin.net/api/" + path
    if params:
        url += "?" + urllib.parse.urlencode(params)
    request = urllib.request.Request(url, headers={
        "webid": WEBID, "Accept": "application/json", "Pragma": "no-cache",
        "User-Agent": "HeilbronnAppointmentAlert/1.0",
    })
    with urllib.request.urlopen(request, timeout=30) as response:
        data = json.load(response)
    if not isinstance(data, list) or any(not isinstance(x, dict) for x in data):
        raise ValueError(f"Unexpected response from {path}; check the booking page")
    return data


def parameters(state, timestamp):
    # Refresh website settings and service details hourly; never bypass lead times.
    cached = state.get("parameters")
    if cached and timestamp - state.get("parameters_at", 0) < 3600:
        return dict(cached)
    settings = api("settingbs", {"t": ""})
    if len(settings) != 1:
        raise ValueError("Booking settings unavailable")
    settings = settings[0]
    services = api("servicegroupservice", {
        "cache": 1, "w": WEBID, "v": settings["vservices"], "lang": "en",
    })
    service = next((s for s in services if s.get("sid") == SERVICE), None)
    if not service or service.get("s", "").strip() != "extension of a residence permit":
        raise ValueError("Renewal service changed; manual review needed")
    if not service.get("senabled") or not service.get("sgenabled"):
        raise ValueError("Renewal service is disabled")

    def inherited(service_key, setting_key):
        value = service.get(service_key, -1)
        return settings[setting_key] if value == -1 else value

    result = {
        "serviceid": SERVICE, "caching": "false", "capacity": 1,
        "duration": service["duration"],
        "cluster": str(settings["cluster"]).lower(), "slottype": 0,
        "fillcalendarstrategy": inherited("fcs", "fillcalendarstrategy"),
        "showavcap": str(bool(inherited("showavcap", "showavcap"))).lower(),
        "appfuture": settings["appfuture"], "appdeadline": settings["appdeadline"],
        "appdeadlinewm": settings["appdeadlinewm"], "oneoff": "null",
        "msdcm": inherited("msdcm", "msdcm"), "calendarid": "",
    }
    state["parameters"] = result
    state["parameters_at"] = timestamp
    return dict(result)


def available_dates(rows):
    dates = set()
    for row in rows:
        if not {"start", "available", "wl"} <= row.keys():
            raise ValueError("Availability response format changed")
        date = dt.datetime.fromisoformat(row["start"]).date()
        if row["available"] == 1 and row["wl"] == 0:
            dates.add(date)
    return sorted(dates)


def confirmed_times(rows, date, now):
    times = set()
    for row in rows:
        if not {"start", "end", "calendarid", "idandtimeslot", "wl"} <= row.keys():
            raise ValueError("Time-slot response format changed")
        start = dt.datetime.fromisoformat(row["start"]).replace(tzinfo=TZ)
        end = dt.datetime.fromisoformat(row["end"]).replace(tzinfo=TZ)
        # The daily endpoint returns bookable rows (its 'available' field is 0).
        if (start.date() == date and start > now and end > start
                and row["wl"] == 0 and row["calendarid"] > 0
                and row["idandtimeslot"] != "0"):
            times.add(start.strftime("%H:%M"))
    return sorted(times)


def notify(title, message):
    channels = os.environ.get("ALERT_CHANNELS", "mac").split(",")
    failures = []
    receipt = None
    if "telegram" in channels:
        try:
            receipt = send_message(title + "\n\n" + message + "\n\n" + BOOKING_URL,
                                   booking_url=BOOKING_URL)
        except Exception as error:
            failures.append(str(error))
    if "mac" not in channels:
        if failures:
            raise RuntimeError("; ".join(failures))
        if "telegram" not in channels:
            raise ValueError("No notification channel configured")
        return receipt
    # Pass text as arguments, so appointment data cannot become AppleScript code.
    script = '''on run argv
display notification (item 2 of argv) with title (item 1 of argv) sound name "Glass"
end run'''
    try:
        subprocess.run(["/usr/bin/osascript", "-e", script, title, message],
                       check=True, timeout=20, capture_output=True)
    except Exception:
        failures.append("Mac notification failed")
    if failures:
        raise RuntimeError("; ".join(failures))
    return receipt


def run(config, state, force=False):
    now = dt.datetime.now(TZ)
    timestamp = time.time()
    first = max(dt.date.fromisoformat(config["first_date"]), now.date())
    last = dt.date.fromisoformat(config["last_date"])
    state["last_run"] = now.isoformat()
    if first > last:
        state.update(status="expired", message="Appointment window has ended")
        return
    if not force and timestamp < state.get("next_attempt", 0):
        return
    previous_failures = state.get("consecutive_errors", 0)
    try:
        params = parameters(state, timestamp)
        days = available_dates(api("timeslots", {
            **params, "date": first.isoformat(), "rangesearch": 1,
        }))
        state["earliest_available_date"] = days[0].isoformat() if days else None
        candidates = [day for day in days if first <= day <= last]
        matches = []
        for date in candidates[:3]:
            times = confirmed_times(api("timeslots", {
                **params, "date": date.isoformat(),
                "tz": "W. Europe Standard Time", "tzaccount": "W. Europe Standard Time",
            }), date, now)
            if times:
                matches.append({"date": date.isoformat(), "times": times})
                break
        state.update(status="available" if matches else "watching", matches=matches,
                     last_success=now.isoformat(), consecutive_errors=0,
                     next_attempt=timestamp + config["interval_seconds"], error=None)
        state["successful_checks"] = state.get("successful_checks", 0) + 1
        if matches:
            match = matches[0]
            # Repeat after 30 minutes if still open; alert immediately for a new day.
            previous = state.get("notified_date")
            if (previous != match["date"] or
                    timestamp - state.get("notified_at", 0) >= config["reminder_seconds"]):
                message = (f"Date: {match['date']}\nTimes: {', '.join(match['times'][:8])}"
                           " (Europe/Berlin)\nService: extension of a residence permit"
                           "\nBook promptly; availability can change.")
                (ROOT / "OPENING.txt").write_text(message + "\n" + BOOKING_URL + "\n")
                receipt = notify("Residence permit appointment available", message)
                if receipt is not None:
                    state.update(last_notification_at=now.isoformat(), telegram_message_id=receipt)
                if config.get("open_booking_page") and sys.platform == "darwin":
                    subprocess.run(["/usr/bin/open", BOOKING_URL], check=True, timeout=15)
                state.update(notified_date=match["date"], notified_at=timestamp)
        else:
            state.pop("notified_date", None)
    except Exception as error:
        failures = previous_failures + 1
        state.update(status="error", error=f"{type(error).__name__}: {error}",
                     consecutive_errors=failures,
                     next_attempt=timestamp + min(3600, 300 * 2 ** min(failures - 1, 4)))
        if failures >= 3 and timestamp - state.get("error_notified_at", 0) >= 86400:
            try:
                notify("Appointment monitor needs attention",
                       "Three checks failed. Send /status to the Telegram bot or check the monitor logs.")
                state["error_notified_at"] = timestamp
            except Exception:
                pass


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--force", action="store_true")
    parser.add_argument("--test-notification", action="store_true")
    args = parser.parse_args()
    if args.test_notification:
        notify("Heilbronn appointment alerts: test",
               "Monitoring residence permit renewals through 14 November 2026. This is a test.")
        print("Test notification submitted to macOS.")
        return
    with (ROOT / ".lock").open("w") as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            return
        config = read_json(CODE / "config.json")
        state = read_json(ROOT / "status.json", {})
        run(config, state, args.force)
        write_json(ROOT / "status.json", state)
        print(json.dumps({k: v for k, v in state.items() if k != "parameters"}), flush=True)
        if state.get("status") == "error":
            sys.exit(1)


if __name__ == "__main__":
    main()
