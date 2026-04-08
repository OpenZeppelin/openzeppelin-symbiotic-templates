# Example Contracts

`TestOApp.sol` is a disposable LayerZero demo app used by the local LayerZero flow.

It exists to give `make send` and `make e2e` a concrete contract to drive through:

1. `SendUln302`
2. `SymbioticLayerZeroDVN.assignJob`
3. operator batching and BLS signing
4. destination `submitProof`
5. destination receive execution

## Use It

From the repository root:

```bash
make start
make send MSG="hello"
make e2e
```

From `contracts/`:

```bash
forge test --match-path test/examples/TestOApp*.t.sol
```

## Boundaries

- local testing only
- not production integration code
- safe to replace with your own app once you understand the provider flow

For the actual provider contract behavior, see [../../../docs/layerzero.mdx](../../../docs/layerzero.mdx). For operator-side failures, see [../../../docs/troubleshooting.mdx](../../../docs/troubleshooting.mdx).
