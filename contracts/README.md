# Contracts

Solidity contracts for the Symbiotic operator templates.

## What Lives Here

- provider contracts for LayerZero and Chainlink CCV
- shared Symbiotic settlement, key registry, and voting power contracts
- Foundry tests and deployment scripts

## Common Workflows

From the repository root:

```bash
make start
make deploy
make test-contracts
```

From `contracts/` when you only need Foundry:

```bash
forge build
forge test
```

Canonical deployment addresses are written to `../deployments/<env>.json`.

For provider behavior and deployment flow, use the repo-level docs:

- [../docs/layerzero.mdx](../docs/layerzero.mdx)
- [../docs/chainlink-ccv.mdx](../docs/chainlink-ccv.mdx)
- [../docs/deployment.mdx](../docs/deployment.mdx)
