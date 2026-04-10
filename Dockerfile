FROM node:24-alpine AS front-builder
WORKDIR /app/frontend
COPY frontend/package.json frontend/package-lock.json ./
RUN npm ci --ignore-scripts
COPY frontend/ ./
RUN npm run build

FROM rust:1.88.0-bookworm AS backend-builder
WORKDIR /app
RUN apt-get update \
    && apt-get install -y --no-install-recommends musl-tools ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY Cargo.toml Cargo.lock rustfmt.toml ./
COPY .cargo/ ./.cargo/
COPY packaging/ ./packaging/
COPY patches/ ./patches/
COPY vendor/ ./vendor/
COPY crates/ ./crates/
RUN rustup target add x86_64-unknown-linux-musl \
    && if [ -x /app/packaging/docker/sui ]; then \
      cp /app/packaging/docker/sui /app/sui; \
    else \
      cargo build --offline --release --target x86_64-unknown-linux-musl -p app; \
      cp /app/target/x86_64-unknown-linux-musl/release/app /app/sui; \
    fi

FROM alpine:3.23 AS singbox-fetcher
ARG SING_BOX_VERSION=1.13.5
WORKDIR /fetch
RUN apk add --no-cache ca-certificates wget tar
COPY scripts/fetch-sing-box.sh /usr/local/bin/fetch-sing-box
RUN sh /usr/local/bin/fetch-sing-box linux amd64 /opt/sing-box "${SING_BOX_VERSION}"

FROM alpine:3.23
LABEL org.opencontainers.image.title="YT HOME RUST"
LABEL org.opencontainers.image.description="Rust control plane for sing-box based home access."
LABEL org.opencontainers.image.source="https://github.com/YTjungle666/YT-HOME-RUST"
LABEL org.opencontainers.image.licenses="GPL-3.0-only"
ENV SUI_WEB_DIR=/app/web
ENV SUI_MIGRATIONS_DIR=/app/migrations
WORKDIR /app
RUN apk add --no-cache ca-certificates tzdata openrc \
    && mkdir -p /app/db
COPY --chmod=755 --from=backend-builder /app/sui /app/sui
COPY --chmod=755 --from=singbox-fetcher /opt/sing-box/ /app/
COPY --from=front-builder /app/frontend/dist/ /app/web/
COPY crates/infra-db/migrations/ /app/migrations/
COPY --chmod=755 packaging/openrc/sui /etc/init.d/s-ui
RUN rc-update add s-ui default
EXPOSE 80 2096
ENTRYPOINT ["/app/sui"]
