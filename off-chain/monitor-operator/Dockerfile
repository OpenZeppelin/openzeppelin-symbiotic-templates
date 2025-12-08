# Symbiotic LayerZero DVN Monitor Operator
# Extends OpenZeppelin Monitor with custom DVN worker binary
#
# Build: docker build -t symbiotic-dvn-monitor .
# Run:   docker run -v ./config:/app/config symbiotic-dvn-monitor

# =============================================================================
# Stage 1: Build DVN Worker Binary
# =============================================================================
FROM rust:1.85-bookworm AS dvn-builder

# Install protobuf compiler for tonic-build
RUN apt-get update && apt-get install -y protobuf-compiler && rm -rf /var/lib/apt/lists/*

WORKDIR /build

# Copy worker source
COPY workers/layerzero_dvn_worker/Cargo.toml workers/layerzero_dvn_worker/Cargo.lock* ./
COPY workers/layerzero_dvn_worker/build.rs ./
COPY workers/layerzero_dvn_worker/proto ./proto
COPY workers/layerzero_dvn_worker/src ./src

# Build release binary
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/build/target \
    cargo build --release && \
    cp target/release/layerzero_dvn_worker /layerzero_dvn_worker

# =============================================================================
# Stage 2: Extend OZ Monitor with DVN Worker
# =============================================================================
FROM openzeppelin/openzeppelin-monitor:latest

USER root

# Copy DVN worker binary (--chmod ensures executable regardless of local permissions)
COPY --from=dvn-builder --chmod=755 /layerzero_dvn_worker /app/layerzero_dvn_worker

# Copy configuration files and make config directory writable for dynamic config generation
COPY config /app/config
RUN chown -R nonroot:nonroot /app/config

# Copy entrypoint and trigger scripts with execute permissions
COPY --chmod=755 entrypoint.sh /app/entrypoint.sh
COPY --chmod=755 config/triggers/scripts/*.sh /app/config/triggers/scripts/

USER nonroot

ENTRYPOINT ["/app/entrypoint.sh"]
