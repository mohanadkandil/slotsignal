"""Minimal Telegram client with errors that never include the secret request URL."""
import json
import os
import re
import urllib.error
import urllib.request
from pathlib import Path


def load_env(path):
    """Read this project's plain KEY=value file; never execute shell content."""
    if not Path(path).exists():
        return
    for line in Path(path).read_text().splitlines():
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        key, separator, value = line.partition("=")
        if not separator or not re.fullmatch(r"[A-Z][A-Z0-9_]*", key):
            raise ValueError("Invalid environment file format")
        os.environ.setdefault(key, value)


def call(method, payload=None, timeout=35):
    token = os.environ.get("TELEGRAM_BOT_TOKEN", "")
    if not re.fullmatch(r"\d+:[A-Za-z0-9_-]+", token):
        raise RuntimeError("Telegram bot token is missing or invalid")
    request = urllib.request.Request(
        f"https://api.telegram.org/bot{token}/{method}",
        data=json.dumps(payload or {}).encode(),
        headers={"Content-Type": "application/json"}, method="POST",
    )
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            data = json.load(response)
    except urllib.error.HTTPError as error:
        # Do not stringify HTTPError/URLError: they can include /bot<TOKEN>/.
        raise RuntimeError(f"Telegram rejected {method} (HTTP {error.code})") from None
    except Exception:
        raise RuntimeError(f"Telegram network request failed: {method}") from None
    if not isinstance(data, dict) or data.get("ok") is not True:
        raise RuntimeError(f"Telegram did not accept {method}")
    return data["result"]


def send_message(text, booking_url=None):
    chat_id = os.environ.get("TELEGRAM_CHAT_ID", "")
    if not re.fullmatch(r"[1-9]\d*", chat_id):
        raise RuntimeError("A private Telegram chat must be paired first")
    payload = {"chat_id": chat_id, "text": text,
               "link_preview_options": {"is_disabled": True}}
    if booking_url:
        payload["reply_markup"] = {"inline_keyboard": [[
            {"text": "Open renewal booking", "url": booking_url}
        ]]}
    result = call("sendMessage", payload)
    return result["message_id"]
