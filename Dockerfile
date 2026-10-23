FROM rust:1.97.1-slim-bookworm AS build
RUN apt-get update && apt-get install -y --no-install-recommends gcc libc6-dev \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo 'fn main() {}' > src/main.rs && cargo build --locked --release
COPY src ./src
RUN touch src/main.rs && cargo build --locked --release

FROM debian:bookworm-slim
RUN groupadd --gid 10001 monitor && useradd --uid 10001 --gid monitor --create-home monitor \
    && mkdir /data && chown monitor:monitor /data
WORKDIR /app
COPY --from=build /build/target/release/slotsignal /usr/local/bin/slotsignal
COPY config.json ./
ENV ALERT_DATA_DIR=/data ALERT_HEALTH_PORT=8080
HEALTHCHECK --interval=60s --timeout=10s --start-period=180s --retries=3 \
    CMD ["slotsignal", "healthcheck"]
CMD ["slotsignal", "serve"]
