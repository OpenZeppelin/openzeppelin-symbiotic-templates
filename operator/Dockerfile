# syntax=docker/dockerfile:1
# Build stage - use native architecture
FROM rust:1.92-alpine AS builder

# Install build dependencies
RUN apk add --no-cache musl-dev protobuf-dev clang llvm openssl-dev openssl-libs-static

WORKDIR /app

# Copy all source files
COPY . .

# Build with BuildKit cache mounts for cargo registry, git, and target directory
# Note: We copy the binary out because cache mounts don't persist to the image
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/app/target \
    cargo build --release && \
    cp target/release/operator /operator-bin

# Runtime stage - use alpine for smaller image with runtime support
FROM alpine:latest

# Install ca-certificates for HTTPS
RUN apk add --no-cache ca-certificates

# Copy the binary
COPY --from=builder /operator-bin /operator

ENTRYPOINT ["/operator"]
