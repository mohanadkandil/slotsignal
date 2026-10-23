import contextlib
import datetime as dt
import io
import json
import os
from pathlib import Path
import tempfile
import time
import unittest
from unittest.mock import patch
import urllib.error

import monitor as m
import server
import telegram_client as tg


class AlertTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.patch = patch.object(m, "ROOT", self.root)
        self.patch.start()
        self.addCleanup(self.patch.stop)
        self.env = patch.dict(os.environ, {"TELEGRAM_BOT_TOKEN": "12345:FAKE_TEST_TOKEN",
                                          "TELEGRAM_CHAT_ID": "12345", "ALERT_CHANNELS": "telegram"})
        self.env.start()
        self.addCleanup(self.env.stop)
        today = dt.datetime.now(m.TZ).date()
        self.day = today + dt.timedelta(days=2)
        self.config = {"first_date": today.isoformat(),
                       "last_date": (today + dt.timedelta(days=20)).isoformat(),
                       "interval_seconds": 300, "reminder_seconds": 1800,
                       "open_booking_page": False}
        self.days = [{"start": self.day.isoformat() + "T00:00:00", "available": 1, "wl": 0}]
        self.slot = {"start": self.day.isoformat() + "T09:00:00",
                     "end": self.day.isoformat() + "T09:20:00", "calendarid": 123,
                     "idandtimeslot": "123|example", "wl": 0}

    def run_check(self, state, responses, notification=None):
        with patch.object(m, "parameters", return_value={}), \
                patch.object(m, "api", side_effect=responses), \
                patch.object(m, "notify", **(notification or {})) as notify:
            m.run(self.config, state, force=True)
            return notify

    def test_opening_and_duplicate_suppression(self):
        state = {}
        notify = self.run_check(state, [self.days, [self.slot]], {"return_value": 77})
        self.assertEqual(state["status"], "available")
        self.assertEqual(state["telegram_message_id"], 77)
        self.assertIn(self.day.isoformat(), notify.call_args.args[1])
        notify = self.run_check(state, [self.days, [self.slot]])
        notify.assert_not_called()

    def test_outside_window_and_disappeared_slot(self):
        for responses in [[[], []], [self.days, []]]:
            state = {}
            notify = self.run_check(state, responses)
            self.assertEqual(state["status"], "watching")
            notify.assert_not_called()
        after = dt.date.fromisoformat(self.config["last_date"]) + dt.timedelta(days=1)
        rows = [{"start": after.isoformat()+"T00:00:00", "available": 1, "wl": 0}]
        notify = self.run_check({}, [rows])
        notify.assert_not_called()

    def test_failed_delivery_retries_and_counts_failures(self):
        state = {}
        for _ in range(3):
            self.run_check(state, [self.days, [self.slot]],
                           {"side_effect": RuntimeError("Telegram rejected sendMessage (HTTP 403)")})
        self.assertEqual(state["status"], "error")
        self.assertEqual(state["consecutive_errors"], 3)
        self.assertNotIn("notified_date", state)

    def test_malformed_response_and_waitlist(self):
        with self.assertRaises(ValueError):
            m.available_dates([{"oops": True}])
        self.assertEqual(m.available_dates([{**self.days[0], "wl": 1}]), [])
        self.assertEqual(m.confirmed_times([{**self.slot, "wl": 1}], self.day,
                                           dt.datetime.now(m.TZ)), [])

    def test_expiry_never_requests(self):
        config = {**self.config, "last_date": "2000-01-01"}
        state = {}
        with patch.object(m, "api") as request:
            m.run(config, state, True)
        request.assert_not_called()
        self.assertEqual(state["status"], "expired")

    def test_send_message_has_link_and_button(self):
        with patch.object(tg, "call", return_value={"message_id": 7}) as call:
            self.assertEqual(tg.send_message("Date and times", m.BOOKING_URL), 7)
        payload = call.call_args.args[1]
        self.assertEqual(payload["chat_id"], "12345")
        self.assertEqual(payload["reply_markup"]["inline_keyboard"][0][0]["url"], m.BOOKING_URL)

    def test_transport_errors_redact_secret(self):
        token = os.environ["TELEGRAM_BOT_TOKEN"]
        url = "https://api.telegram.org/bot" + token + "/sendMessage"
        error = urllib.error.HTTPError(url, 403, "forbidden", {}, None)
        with patch.object(tg.urllib.request, "urlopen", side_effect=error):
            with self.assertRaises(RuntimeError) as raised:
                tg.call("sendMessage", {})
        self.assertNotIn(token, str(raised.exception))
        self.assertNotIn("https://", str(raised.exception))

    def command(self, text, chat=12345, user=12345, kind="private"):
        return {"update_id": 10, "message": {"text": text, "date": time.time(),
                "chat": {"id": chat, "type": kind}, "from": {"id": user}}}

    def test_unauthorized_chat_never_receives_response(self):
        with patch.object(server, "send_message") as send, patch.object(server, "test_message") as test:
            for update in [self.command("/status", chat=55), self.command("/test", user=66),
                           self.command("/status", kind="group")]:
                server.handle_update(update, self.config, {})
        send.assert_not_called()
        test.assert_not_called()

    def test_status_and_live_check_commands(self):
        state = {}
        with patch.object(server, "send_message") as send, patch.object(m, "run") as run:
            server.handle_update(self.command("/status"), self.config, state)
            self.assertIn("Last successful availability check", send.call_args.args[0])
            server.handle_update(self.command("/check"), self.config, state)
            server.handle_update(self.command("/check"), self.config, state)
            self.assertEqual(run.call_count, 1)

    def test_example_alert_is_clearly_labeled(self):
        with patch.object(m, "notify") as notify:
            server.test_message(self.config)
        self.assertIn("TEST ONLY", notify.call_args.args[0])
        self.assertIn("not an available appointment", notify.call_args.args[1])

    def test_health_requires_recent_website_and_telegram(self):
        now = dt.datetime.now(m.TZ).isoformat()
        state = {"loop_at": time.time(), "status": "watching", "last_success": now,
                 "last_telegram_poll": now}
        m.write_json(self.root / "status.json", state)
        self.assertEqual(server.healthcheck(), 0)
        state["last_success"] = (dt.datetime.now(m.TZ) - dt.timedelta(hours=1)).isoformat()
        m.write_json(self.root / "status.json", state)
        self.assertEqual(server.healthcheck(), 1)
        state["last_success"] = now
        state.pop("last_telegram_poll")
        m.write_json(self.root / "status.json", state)
        self.assertEqual(server.healthcheck(), 1)


if __name__ == "__main__":
    unittest.main()
