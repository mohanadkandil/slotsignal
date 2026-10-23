"""Initialize Fly's root-owned data volume, then drop privileges before running."""
import os
from pathlib import Path
import sys

data = Path(os.environ.get("ALERT_DATA_DIR", "/data"))
data.mkdir(parents=True, exist_ok=True)
if os.geteuid() == 0:
    os.chown(data, 10001, 10001)
    os.setgroups([])
    os.setgid(10001)
    os.setuid(10001)
os.execv(sys.executable, [sys.executable, "/app/server.py", *sys.argv[1:]])
