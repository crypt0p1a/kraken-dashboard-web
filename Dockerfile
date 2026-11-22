# kraken-dashboard-web
#
#   docker compose up -d --build
#
# The image only holds the binary and the page: no shell, no package manager, a non-root user
# (65532).

# ---- the page: the dashboard in WebAssembly, built by trunk ----
# trixie: the wasm-bindgen trunk downloads needs its glibc (2.38 at least)
FROM rust:1-trixie AS page
ARG TRUNK_VERSION=0.21.14
RUN rustup target add wasm32-unknown-unknown \
    && archive="trunk-$(uname -m)-unknown-linux-gnu.tar.gz" \
    && base="https://github.com/trunk-rs/trunk/releases/download/v${TRUNK_VERSION}" \
    && curl -fsSLO "${base}/${archive}" \
    && echo "$(curl -fsSL "${base}/${archive}.sha256" | awk '{print $1}')  ${archive}" | sha256sum -c - \
    && tar -xzf "${archive}" -C /usr/local/bin trunk \
    && rm "${archive}"
WORKDIR /build
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/build/target \
    cd front && trunk build --release \
    && cp -r dist /page

# ---- the backend ----
FROM rust:1-bookworm AS builder
WORKDIR /build
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/build/target \
    cargo build --release --locked -p kraken-dashboard-web \
    && cp target/release/kraken-dashboard-web /kraken-dashboard-web

# ---- run ----
FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=builder /kraken-dashboard-web /usr/local/bin/kraken-dashboard-web
COPY --from=page /page /usr/share/kraken-dashboard-web/page
# every interface of its container: reached through the port Docker publishes
ENV KRAKEN_DASHBOARD_WEB_LISTEN=0.0.0.0:8080 \
    KRAKEN_DASHBOARD_WEB_PAGE=/usr/share/kraken-dashboard-web/page \
    RUST_LOG=info
EXPOSE 8080
USER 65532:65532
ENTRYPOINT ["/usr/local/bin/kraken-dashboard-web"]
