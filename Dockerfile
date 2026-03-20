FROM rust:1.80-slim as builder

WORKDIR /app

# Copy Cargo manifest only
COPY Cargo.toml ./

# Create dummy target to satisfy Cargo's manifest parser
RUN mkdir src && echo "fn main() {}" > src/main.rs

# Generate fresh lockfile inside container
RUN cargo generate-lockfile

# Copy source code
COPY src ./src

# Build optimized binary
RUN cargo build --release


# Runtime stage (lightweight)
FROM debian:bookworm-slim

WORKDIR /app

RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/solana_sniper_bot .

# Run directly (no shell)
ENTRYPOINT ["./solana_sniper_bot"]
