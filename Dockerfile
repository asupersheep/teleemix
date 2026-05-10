# Build stage
FROM rust:latest AS builder

RUN apt-get update && apt-get install -y musl-tools && rm -rf /var/lib/apt/lists/*
RUN rustup target add x86_64-unknown-linux-musl

WORKDIR /app
COPY Cargo.toml Cargo.lock* ./
COPY src ./src

RUN cargo build --release --target x86_64-unknown-linux-musl

# Docker CLI stage - grab just the docker binary
FROM docker:latest AS docker-cli

# Runtime stage
FROM scratch

# CA certificates for HTTPS
COPY --from=builder /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/

# Docker CLI for /updatearl rebuild trigger
COPY --from=docker-cli /usr/local/bin/docker /usr/local/bin/docker

# The bot binary
COPY --from=builder /app/target/x86_64-unknown-linux-musl/release/teleemix /teleemix

CMD ["/teleemix"]
