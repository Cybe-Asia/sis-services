# 1. Build stage
FROM rust:1.91 as builder

WORKDIR /app

COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main(){}" > src/main.rs
RUN cargo build --release
# Remove dummy build artifacts to force Cargo to rebuild our actual app
RUN rm -rf src target/release/deps/sis_service* target/release/sis-service* target/release/.fingerprint/sis_service* || true

COPY . .
RUN cargo build --release

# 2. Runtime stage
#
# Same base as admission-services — debian:trixie-slim ships glibc 2.39
# which covers the rust:1.91 builder's glibc 2.38 requirement.
FROM debian:trixie-slim

WORKDIR /app

# Install ca-certificates required for HTTPS requests via reqwest
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/sis-service .

EXPOSE 8081

CMD ["./sis-service"]
