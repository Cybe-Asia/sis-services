# 1. Build stage
FROM rust:1.91 as builder

WORKDIR /app

# No Cargo.lock yet — first build will generate it and we commit it
# after. Until then, skip the layer-caching trick and build directly.
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
