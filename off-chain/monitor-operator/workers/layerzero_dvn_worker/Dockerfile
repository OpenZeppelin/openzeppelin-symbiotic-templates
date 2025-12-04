# Build stage
FROM rust:1.91-bookworm AS builder

# Install protobuf compiler for tonic-build
RUN apt-get update && apt-get install -y protobuf-compiler && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy manifests and build.rs
COPY Cargo.toml Cargo.lock* build.rs ./

# Copy proto files (needed by build.rs)
COPY proto ./proto

# Create dummy source to cache dependencies
RUN mkdir src && echo "fn main() {}" > src/main.rs
RUN cargo build --release
RUN rm -rf src

# Copy actual source
COPY src ./src

# Build for release
RUN touch src/main.rs && cargo build --release

# Runtime stage
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    jq \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /app/target/release/layerzero_dvn_worker /app/layerzero_dvn_worker

ENV RUST_LOG=dvn_worker=info

ENTRYPOINT ["/app/layerzero_dvn_worker"]
