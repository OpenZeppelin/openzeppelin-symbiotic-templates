# Monitor Operator

Rust DVN worker consumes OZ Monitor events (`PacketSent`, `DVNFeePaid`), calls Symbiotic sidecars to sign payload hashes, and submits proofs to `SymbioticLayerZeroDVN` on destination chains.

## Layout

- `config/` – OZ Monitor JSON configs for devnet chains.
- `workers/layerzero_dvn_worker/` – Rust binary (Cargo) that polls PacketSent events.

## Dev

```bash
cd workers/layerzero_dvn_worker
cargo run -- --rpc-app-a http://localhost:8545 --rpc-app-b http://localhost:8546 --rpc-settlement http://localhost:8547
```
