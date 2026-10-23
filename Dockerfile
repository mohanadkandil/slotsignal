FROM python:3.12-slim
RUN apt-get update && apt-get install -y --no-install-recommends tzdata ca-certificates \
    && rm -rf /var/lib/apt/lists/*
RUN groupadd --gid 10001 monitor && useradd --uid 10001 --gid monitor --create-home monitor \
    && mkdir /data && chown monitor:monitor /data
WORKDIR /app
COPY monitor.py telegram_client.py server.py config.json ./
ENV PYTHONUNBUFFERED=1 ALERT_DATA_DIR=/data ALERT_CHANNELS=telegram
USER monitor
HEALTHCHECK --interval=60s --timeout=10s --start-period=180s --retries=3 \
    CMD ["python", "server.py", "--healthcheck"]
CMD ["python", "server.py"]
