FROM node:24-bookworm-slim AS front-builder
WORKDIR /app/frontend
COPY frontend/package.json frontend/package-lock.json ./
RUN npm ci --ignore-scripts
COPY frontend/ ./
RUN npm run build

FROM rust:1.88.0-bookworm AS backend-builder
WORKDIR /app
COPY Cargo.toml Cargo.lock rustfmt.toml ./
COPY .cargo/ ./.cargo/
COPY packaging/ ./packaging/
COPY patches/ ./patches/
COPY vendor/ ./vendor/
COPY crates/ ./crates/
RUN if [ -x /app/packaging/docker/sui ]; then \
      cp /app/packaging/docker/sui /app/sui; \
    else \
      cargo build --offline --release -p app; \
      cp /app/target/release/app /app/sui; \
    fi

FROM alpine:3.22 AS singbox-fetcher
ARG SING_BOX_VERSION=1.13.5
ARG TARGETARCH
ARG TARGETVARIANT
WORKDIR /fetch
RUN apk add --no-cache ca-certificates wget tar
COPY scripts/fetch-sing-box.sh /usr/local/bin/fetch-sing-box
RUN sh /usr/local/bin/fetch-sing-box linux "${TARGETARCH}${TARGETVARIANT}" /opt/sing-box "${SING_BOX_VERSION}"

FROM debian:bookworm-slim
LABEL org.opencontainers.image.title="YT HOME RUST"
LABEL org.opencontainers.image.description="Rust control plane for sing-box based home access."
LABEL org.opencontainers.image.source="https://github.com/YTjungle666/YT-HOME-RUST"
LABEL org.opencontainers.image.licenses="GPL-3.0-only"
ENV SUI_WEB_DIR=/app/web
ENV SUI_MIGRATIONS_DIR=/app/migrations
WORKDIR /app
RUN mkdir -p /app/db
COPY --from=backend-builder /app/sui /app/sui
COPY --from=backend-builder /etc/ssl/certs/ /etc/ssl/certs/
COPY --from=singbox-fetcher /opt/sing-box/ /app/
COPY --from=front-builder /app/frontend/dist/ /app/web/
COPY crates/infra-db/migrations/ /app/migrations/
COPY packaging/ct-init.sh /sbin/init
RUN chmod +x /app/sui /app/sing-box /sbin/init
EXPOSE 80 2096
ENTRYPOINT ["/app/sui"]
