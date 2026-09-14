# 1. Build stage
FROM rust:1.91 as builder

WORKDIR /app

# Use the committed lockfile for reproducible dependency resolution.
COPY . .
RUN cargo build --release --locked

# 2. Runtime stage
#
# Same base as admission-services — debian:trixie-slim ships glibc 2.39
# which covers the rust:1.91 builder's glibc 2.38 requirement.
FROM debian:trixie-slim

WORKDIR /app

# Install HTTPS trust roots and the security-patched Perl base package.
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates perl-base && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/sis-service .

EXPOSE 8081

CMD ["./sis-service"]
