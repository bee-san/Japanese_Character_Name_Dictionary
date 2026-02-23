# ---- Builder Stage ----
FROM rust:1.83-bookworm AS builder

WORKDIR /app

# Install OpenSSL dev libraries for native-tls
RUN apt-get update && apt-get install -y pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*

# Cache dependency build: copy manifests first
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo 'fn main() {}' > src/main.rs
RUN cargo build --release && rm -rf src

# Copy actual source and rebuild
COPY src/ src/
# Touch main.rs so cargo detects it changed
RUN touch src/main.rs
RUN cargo build --release

# ---- Runtime Stage ----
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y ca-certificates libssl3 && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy the compiled binary
COPY --from=builder /app/target/release/yomitan-dict-builder .

# Copy static assets
COPY static/ static/

EXPOSE 3000

CMD ["./yomitan-dict-builder"]