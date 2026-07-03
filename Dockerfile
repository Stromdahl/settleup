# syntax=docker/dockerfile:1

# ---- build stage --------------------------------------------------------------
# Full (non-slim) rust image: it ships gcc, which sqlx's `sqlite` feature needs to
# compile the bundled libsqlite3-sys C sources. The `1` tag tracks the latest 1.x
# stable, which satisfies this crate's `edition = "2024"` (needs Rust >= 1.85).
FROM rust:1-bookworm AS build
WORKDIR /app

# Pre-compile dependencies as their own cache layer: this RUN only re-executes when
# Cargo.toml/Cargo.lock change, so ordinary source edits reuse the compiled deps.
COPY Cargo.toml Cargo.lock ./
RUN mkdir src \
 && echo 'fn main() { panic!("dependency cache stub"); }' > src/main.rs \
 && cargo build --release --locked \
 && rm -rf src target/release/settleup target/release/deps/settleup-*

# Build the real binary against the already-compiled dependency cache.
COPY src ./src
RUN cargo build --release --locked

# ---- runtime stage ------------------------------------------------------------
# debian:bookworm-slim matches the builder's glibc. libsqlite3-sys is statically
# linked (bundled), so no SQLite runtime package is required.
FROM debian:bookworm-slim AS runtime
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --system --uid 10001 --shell /usr/sbin/nologin app \
 && mkdir -p /data && chown app:app /data

COPY --from=build /app/target/release/settleup /usr/local/bin/settleup

# Bind all interfaces (the container is only reached via its internal network) and
# keep the SQLite file on the mounted volume so data survives container replacement.
# SETTLEUP_BASE_URL is intentionally left to the deployment (it is site-specific).
ENV SETTLEUP_ADDR=0.0.0.0:3000 \
    SETTLEUP_DB=/data/settleup.db

EXPOSE 3000
# Persistence point. chown above runs before this, so a fresh named volume inits
# owned by `app` and the non-root process can write to it.
VOLUME ["/data"]
USER app
ENTRYPOINT ["/usr/local/bin/settleup"]
