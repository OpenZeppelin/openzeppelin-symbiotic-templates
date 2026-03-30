# Documentation

## For Operators

Running, configuring, and monitoring the stack.

1. [Setup](setup.md) -- Config structure, environment setup, running locally
2. [Deployment](deployment.md) -- Testnet and mainnet deployment
3. Choose your provider:
   - [LayerZero](layerzero.md) -- DVN for LayerZero V2
   - [Chainlink CCV](chainlink-ccv.md) -- Cross-Chain Verifier for CCIP
4. [CLI & API Reference](cli.md) -- Commands, HTTP endpoints, webhook config
5. [Troubleshooting](troubleshooting.md) -- Common issues and debugging

## For Integrators

Understanding the system and adding new providers.

1. [Architecture](architecture.md) -- Provider model, shared infra, Merkle batching, BLS signing
2. Choose your provider:
   - [LayerZero](layerzero.md) -- Message flow, contracts, code pointers
   - [Chainlink CCV](chainlink-ccv.md) -- Message flow, contracts, code pointers
3. [Architecture: Adding a New Provider](architecture.md#adding-a-new-provider) -- Provider trait, registration, templates
4. [Security](security.md) -- Trust model, access control, invariants
