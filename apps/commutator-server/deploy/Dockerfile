# commutator-server for the pets container on kuzyo. Context: the repo root
# (deploy.sh syncs it without target/).
FROM rust:1-bookworm AS build
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p commutator-server \
    && cp target/release/commutator-server /usr/local/bin/

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=build /usr/local/bin/commutator-server /usr/local/bin/
USER 1000:1000
CMD ["commutator-server"]
