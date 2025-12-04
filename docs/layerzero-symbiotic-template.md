# LayerZero × Symbiotic Template

- DVN contract implements `ILayerZeroDVN` ABI and calls Symbiotic settlement to validate payload hashes before touching ReceiveUln.
- Middleware owns reward/slash params, resolvers decode packet payloads for monitoring and future slashing evidence.
- Example OApp + Foundry tests exercise `assignJob`, `getFee`, and `submitVerification` flows off-chain via LayerZero devtools.
