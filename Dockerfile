# Stage 1: Builder
FROM rust:1.75-slim AS builder

WORKDIR /usr/src/app
RUN apt-get update && apt-get install -y pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*

# Cache dependencies
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main() {}" > src/main.rs
RUN cargo build --release
RUN rm -f target/release/deps/solana_sniper_bot*

# Build real binary
COPY . .
RUN cargo build --release

# Stage 2: Runtime
FROM debian:bookworm-slim AS runtime

WORKDIR /app
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*

COPY --from=builder /usr/src/app/target/release/solana_sniper_bot .

# Standardize logs to stdout
ENV RUST_LOG=warn
ENV PORT=8080

EXPOSE 8080
ENTRYPOINT ["./solana_sniper_bot"]
