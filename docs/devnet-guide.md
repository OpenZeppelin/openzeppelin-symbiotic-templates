# Devnet Guide

1. `pnpm install && forge build`.
2. `cd devnet && ./scripts/up.sh` → spins 2 Anvil nodes (app A + app B/settlement), relay sidecars, aggregator, OZ Monitors, and the Rust DVN worker.
3. Use generated `devnet/deploy-data/*.json` to configure workers and OZ Monitor env vars.
4. Fire a test message: `forge script script/SendExample.s.sol --rpc-url http://localhost:8545 --broadcast`.
5. Tear down: `./scripts/down.sh`.
