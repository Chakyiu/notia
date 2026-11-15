# Build context is the repository root. See docker/sync/compose.yaml.
# 1.88 or newer: the sources use let chains, which 1.85 rejects.
FROM rust:1.100-bookworm AS build

WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN cargo build --locked --release \
    -p notia-sync --bin notia-sync-server \
    --no-default-features --features server

FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends curl \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --home-dir /data --create-home notia

COPY --from=build /src/target/release/notia-sync-server /usr/local/bin/notia-sync-server

USER notia
WORKDIR /data
EXPOSE 8787
VOLUME ["/data"]

ENTRYPOINT ["notia-sync-server"]
CMD ["--data", "/data", "--listen", "0.0.0.0:8787"]
