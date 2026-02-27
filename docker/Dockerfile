# ── Stage 1: Build ───────────────────────────────────────────────
FROM rust:1.85-bookworm AS builder

RUN apt-get update && apt-get install -y --no-install-recommends \
        libssl-dev \
        libsqlite3-dev \
        pkg-config \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build
COPY . .
RUN cargo build --release --all-features --bin sentinel \
    && strip target/release/sentinel

# ── Stage 2: Runtime ─────────────────────────────────────────────
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates \
        libssl3 \
        libsqlite3-0 \
        libdbus-1-3 \
        tini \
    && rm -rf /var/lib/apt/lists/*

# Unprivileged user
RUN groupadd -r sentinel && useradd -r -g sentinel -m sentinel

COPY --from=builder /build/target/release/sentinel /usr/local/bin/sentinel

# Data and config volumes
RUN mkdir -p /home/sentinel/.local/share/sentinel \
             /home/sentinel/.config/sentinel \
    && chown -R sentinel:sentinel /home/sentinel

# Inside Docker the dashboard must listen on all interfaces
ENV SENTINEL_DASHBOARD_BIND=0.0.0.0

USER sentinel
WORKDIR /home/sentinel

ENTRYPOINT ["tini", "--"]
CMD ["sentinel", "run", "--dashboard-port", "8765"]

# Dashboard
EXPOSE 8765

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD ["sentinel", "check"]

LABEL org.opencontainers.image.source="https://github.com/your-org/sentinel" \
      org.opencontainers.image.description="Sentinel — self-hosted personal AI assistant" \
      org.opencontainers.image.licenses="MIT"
