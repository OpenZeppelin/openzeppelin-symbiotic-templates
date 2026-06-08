# Example Contracts

`ExampleOApp.sol` is the built-in LayerZero starter app used by the template messaging flow.

It exists to give `make send` and `make e2e` a concrete contract to drive through:

1. `SendUln302`
2. `SymbioticLayerZeroDVN.assignJob`
3. operator batching and BLS signing
4. destination `submitProof`
5. destination receive execution

`ExampleCcipApp.sol` is the equivalent starter app for the Chainlink CCV provider (`activeProvider: chainlink_ccv`). It sends through the CCIP Router secured by the Symbiotic CCV and is what `make send` / `make e2e` drive on the `chainlink_ccv` path (for example the `testnet-ccv` environment). `NoOpExecutor.sol` is the source-side no-op executor used by that path.

## Use It

From the repository root:

```bash
make start
make send MSG="hello"
make e2e
```

From `contracts/`:

```bash
forge test --match-path test/examples/ExampleOApp*.t.sol
```

## Boundaries

- starter application code for the template
- safe to customize or replace with your own app once you understand the provider flow
- not required provider infrastructure for LayerZero validation

For the actual provider contract behavior, see [../../../docs/layerzero.mdx](../../../docs/layerzero.mdx). For operator-side failures, see [../../../docs/troubleshooting.mdx](../../../docs/troubleshooting.mdx).
