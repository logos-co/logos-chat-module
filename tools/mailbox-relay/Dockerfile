# Multi-stage build for the mailbox relay. rusqlite is `bundled`, so sqlite is
# compiled and statically linked into the binary — the runtime image needs no
# libsqlite3. `fly launch` uses a remote builder, so this never compiles locally.
FROM rust:1-slim AS build
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY tests ./tests
RUN cargo build --release --locked --bin mailbox-relay

FROM debian:stable-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /app/target/release/mailbox-relay /usr/local/bin/mailbox-relay
# Persisted on a Fly volume mounted at /data (see README).
ENV RELAY_DB=/data/relay.db
ENV RELAY_BIND=0.0.0.0:8080
EXPOSE 8080
CMD ["mailbox-relay"]
