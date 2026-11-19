FROM rust:bookworm AS builder

WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY xtask ./xtask

RUN cargo build --release -p abmx --bin abmx

# better-sqlite3 is a native module. Build it once with a toolchain, then copy
# only the result into the runtime image.
FROM node:22-bookworm-slim AS helper-deps

RUN apt-get update \
    && apt-get install -y --no-install-recommends python3 make g++ \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY crates/abmx-actual/helper/package.json crates/abmx-actual/helper/package-lock.json ./
ENV npm_config_build_from_source=true
RUN npm ci --omit=dev

FROM node:22-bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY --from=helper-deps /app/node_modules ./node_modules
COPY crates/abmx-actual/helper/helper.js ./

COPY --from=builder /src/target/release/abmx /usr/local/bin/abmx

ENV ABMX_ACTUAL_HELPER=/app/helper.js \
    ABMX_ACTUAL_DATA_DIR=/data/cache/actual \
    XDG_CONFIG_HOME=/data/config \
    XDG_STATE_HOME=/data/state \
    XDG_CACHE_HOME=/data/cache

VOLUME /data

ENTRYPOINT ["abmx"]
