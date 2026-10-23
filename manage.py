#!/usr/bin/python3
"""Install/manage a per-user macOS LaunchAgent; no administrator access needed."""
import argparse
import json
import os
import plistlib
import shutil
import subprocess
from pathlib import Path

SOURCE = Path(__file__).resolve().parent
RUNTIME = Path.home() / "Library/Application Support/HeilbronnAppointmentAlert"
LABEL = "local.heilbronn.residence-permit-alert"
PLIST = Path.home() / "Library/LaunchAgents" / (LABEL + ".plist")
DOMAIN = f"gui/{os.getuid()}"


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("action", choices=["install", "stop", "status", "check", "test-notification"])
    args = parser.parse_args()
    if args.action == "install":
        config = json.loads((SOURCE / "config.json").read_text())
        if config["interval_seconds"] < 300:
            raise SystemExit("Use an interval of at least five minutes.")
        RUNTIME.mkdir(parents=True, exist_ok=True)
        PLIST.parent.mkdir(parents=True, exist_ok=True)
        subprocess.run(["launchctl", "bootout", DOMAIN + "/" + LABEL], capture_output=True)
        for filename in ["monitor.py", "telegram_client.py", "config.json"]:
            shutil.copy2(SOURCE / filename, RUNTIME / filename)
        plist = {
            "Label": LABEL,
            "ProgramArguments": ["/usr/bin/python3", str(RUNTIME / "monitor.py")],
            "StartInterval": config["interval_seconds"], "RunAtLoad": True,
            "WorkingDirectory": str(RUNTIME),
            "StandardOutPath": str(RUNTIME / "monitor.log"),
            "StandardErrorPath": str(RUNTIME / "error.log"),
            "ProcessType": "Background",
        }
        PLIST.write_bytes(plistlib.dumps(plist))
        subprocess.run(["launchctl", "bootstrap", DOMAIN, str(PLIST)], check=True)
        print(f"Installed: checks every {config['interval_seconds']} seconds while logged in and awake.")
        print(f"Runtime/status: {RUNTIME}")
    elif args.action == "stop":
        subprocess.run(["launchctl", "bootout", DOMAIN + "/" + LABEL], capture_output=True)
        PLIST.unlink(missing_ok=True)
        print("Stopped and removed the scheduled agent.")
    elif args.action == "status":
        result = subprocess.run(["launchctl", "print", DOMAIN + "/" + LABEL],
                                capture_output=True, text=True)
        print("Scheduler: installed" if result.returncode == 0 else "Scheduler: not installed")
        path = RUNTIME / "status.json"
        state = json.loads(path.read_text()) if path.exists() else {}
        state.pop("parameters", None)
        print(json.dumps(state, indent=2))
    else:
        flag = "--force" if args.action == "check" else "--test-notification"
        subprocess.run(["/usr/bin/python3", str(RUNTIME / "monitor.py"), flag], check=True)


if __name__ == "__main__":
    main()
