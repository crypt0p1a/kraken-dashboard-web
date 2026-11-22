# kraken-dashboard-web
#
#   docker compose up -d --build
#
# The image only holds the binary: no shell, no package manager, a non-root user (65532).

# ---- build ----
FROM rust:1-bookworm AS builder
WORKDIR /build
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/build/target \
    cargo build --release --locked \
    && cp target/release/kraken-dashboard-web /kraken-dashboard-web

# ---- run ----
FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=builder /kraken-dashboard-web /usr/local/bin/kraken-dashboard-web
# every interface of its container: reached through the port Docker publishes
ENV KRAKEN_DASHBOARD_WEB_LISTEN=0.0.0.0:8080 \
    RUST_LOG=info
EXPOSE 8080
USER 65532:65532
ENTRYPOINT ["/usr/local/bin/kraken-dashboard-web"]
