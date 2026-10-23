#!/usr/bin/env python3
"""Pair only the private Telegram chat that presents a random setup challenge."""
import argparse
import hmac
import json
import os
from pathlib import Path
import secrets
import sys
import time

from telegram_client import call, load_env

ROOT = Path(__file__).resolve().parent
PAIRING = ROOT / ".pairing.json"


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("action", choices=["identify", "link", "pair"])
    args = parser.parse_args()
    load_env(ROOT / ".env")
    me = call("getMe")
    if call("getWebhookInfo").get("url"):
        raise RuntimeError("This bot has a webhook. Create a new dedicated bot instead.")
    if args.action == "identify":
        print("Verified bot: @" + me["username"])
    elif args.action == "link":
        data = {"nonce": secrets.token_urlsafe(24), "created_at": time.time()}
        fd = os.open(PAIRING, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
        with os.fdopen(fd, "w") as file:
            json.dump(data, file)
        print(f"https://t.me/{me['username']}?start={data['nonce']}")
    else:
        data = json.loads(PAIRING.read_text())
        if time.time() - data["created_at"] > 600:
            raise RuntimeError("Pairing expired. Restart the setup helper.")
        expected = "/start " + data["nonce"]
        offset = 0
        deadline = time.time() + 180
        print("Waiting for you to press Start in your bot's private chat...", file=sys.stderr)
        while time.time() < deadline:
            updates = call("getUpdates", {"offset": offset, "timeout": 20,
                                          "allowed_updates": ["message"]})
            for update in updates:
                offset = update["update_id"] + 1
                message = update.get("message", {})
                chat = message.get("chat", {})
                if (chat.get("type") == "private" and chat.get("id", 0) > 0
                        and message.get("from", {}).get("id") == chat["id"]
                        and message.get("date", 0) >= data["created_at"] - 5
                        and hmac.compare_digest(message.get("text", "").encode(), expected.encode())):
                    print(chat["id"])
                    PAIRING.unlink(missing_ok=True)
                    return
        raise RuntimeError("No matching Start message received. Run the helper again.")


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        print(f"Setup stopped: {error}", file=sys.stderr)
        sys.exit(1)
