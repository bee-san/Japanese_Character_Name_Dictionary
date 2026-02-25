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
# Distroless: no shell, no package manager, no OS utilities — minimal attack surface
FROM gcr.io/distroless/cc-debian12:nonroot

WORKDIR /app

# Copy the compiled binary
COPY --from=builder /app/target/release/yomitan-dict-builder .

# Copy static assets
COPY static/ static/

EXPOSE 3000

ENV PORT=3000
# Bind to localhost only — Caddy on the host handles external traffic
ENV HOST=127.0.0.1

# Run as nonroot user (uid 65534) built into distroless
USER nonroot

CMD ["./yomitan-dict-builder"]
