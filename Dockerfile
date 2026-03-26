FROM rust:1-bookworm AS builder

WORKDIR /app

COPY Cargo.toml build.rs ./
COPY proto ./proto
COPY migrations ./migrations
COPY src ./src

RUN cargo build --release

FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /app/target/release/fluxa-backend /usr/local/bin/fluxa-backend

EXPOSE 8080 50051

CMD ["fluxa-backend", "--mode", "api"]
