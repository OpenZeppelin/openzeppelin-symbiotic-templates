# Vendored LayerZero V2 Solidity sources

This directory holds a pinned, source-only copy of the small slice of LayerZero V2
Solidity that this template imports. It exists because the upstream npm packages
declare non-optional `peerDependencies` on `hardhat-deploy` and `@chainlink/contracts-ccip`,
which transitively pull in flagged JavaScript dependencies and an old
`@openzeppelin/contracts@4.x` tree that Forge never compiles. The runtime code is
unaffected — it's a packaging concern — but Dependabot still sees ~37 alerts and
that noise does not transfer cleanly to forks of a template repo.

The vendored slice covers exactly the types, libraries, and contracts our `src/`,
`script/`, and `test/` trees actually import. Anything not on the import path —
`OAppRead`, `RateLimiter`, `ReadCodecV1`, `IOAppMapper`, `IOAppReducer`,
`IOAppComposer`, the CCIP DVN adapters, `Proxied.sol`, etc. — is intentionally
not vendored.

## Provenance

| Package                                  | Version | License  | Notes |
|------------------------------------------|---------|----------|-------|
| `@layerzerolabs/lz-evm-messagelib-v2`    | 3.0.153 | LZBL-1.2 | npm tarball SHA-512 below |
| `@layerzerolabs/lz-evm-protocol-v2`      | 3.0.153 | LZBL-1.2 | npm tarball SHA-512 below |
| `@layerzerolabs/oapp-evm`                | 0.4.1   | MIT      | published from public devtools commit |

### `lz-evm-messagelib-v2@3.0.153`
- npm tarball: `https://registry.npmjs.org/@layerzerolabs/lz-evm-messagelib-v2/-/lz-evm-messagelib-v2-3.0.153.tgz`
- npm integrity: `sha512-8XBzrX1Z7cen+ukBF2LNtP0ZKjw8ZFqxO9HO8KbwGz6N0EWtlShLYg+2FOWqO3YF/dKlFMh+A9ifrOcfF1XiGA==`
- npm `gitHead`: `3aed2a54e53735a49fc0206e3631c918abea98f8` *(points at the LayerZero internal release pipeline; not resolvable in the public `LayerZero-Labs/LayerZero-v2` mirror)*
- public mirror: `https://github.com/LayerZero-Labs/LayerZero-v2/tree/main/packages/layerzero-v2/evm/messagelib/contracts`

### `lz-evm-protocol-v2@3.0.153`
- npm tarball: `https://registry.npmjs.org/@layerzerolabs/lz-evm-protocol-v2/-/lz-evm-protocol-v2-3.0.153.tgz`
- npm integrity: `sha512-JNUIrHj25in5S10ZB8KeciRxN7BlSXjBU2NeE4Z5IeHUCcC+w4N+izhZG3NtgsR/LENx/2dyBU8+nHFmqNSBOg==`
- npm `gitHead`: `3aed2a54e53735a49fc0206e3631c918abea98f8` *(same caveat — internal pipeline)*
- public mirror: `https://github.com/LayerZero-Labs/LayerZero-v2/tree/main/packages/layerzero-v2/evm/protocol/contracts`

### `oapp-evm@0.4.1`
- npm tarball: `https://registry.npmjs.org/@layerzerolabs/oapp-evm/-/oapp-evm-0.4.1.tgz`
- npm integrity: `sha512-eOoDepVSrUlVNIlnkH0Vd5Vt4lXBkSBh6Bb16vsLbaN9AryBjy4GDpsE7K4c8iWTFL9BiBXGsV7nJTkgqi+xRQ==`
- public commit: `0306f189c59aa562e3334e35ced8a66ba003f0de` (2025-12-09)
- repo: `https://github.com/LayerZero-Labs/devtools/tree/0306f189c59aa562e3334e35ced8a66ba003f0de/packages/oapp-evm/contracts`

The two `LayerZero-v2` packages publish from a private monorepo, so their
`gitHead` cannot be resolved against a public commit. The npm tarball SHA-512 is
the authoritative pin. The public `LayerZero-Labs/LayerZero-v2` repo mirrors the
same Solidity but is permanently `"private": true` with no version field on its
`package.json`s.

## Files

24 Solidity files, ~1,971 LOC total.

```
messagelib-v2/contracts/
├── MessageLibBase.sol                    LZBL-1.2
├── SendLibBase.sol                       LZBL-1.2  (imports OZ Ownable)
├── interfaces/
│   ├── ILayerZeroExecutor.sol            MIT
│   └── ILayerZeroTreasury.sol            MIT
├── libs/
│   ├── ExecutorOptions.sol               LZBL-1.2
│   └── SafeCall.sol                      MIT OR Apache-2.0
└── uln/
    ├── UlnBase.sol                       LZBL-1.2  (imports OZ Ownable)
    └── libs/DVNOptions.sol               LZBL-1.2  (imports solidity-bytes-utils)

protocol-v2/contracts/
├── interfaces/
│   ├── ILayerZeroEndpointV2.sol          MIT
│   ├── ILayerZeroReceiver.sol            MIT
│   ├── IMessageLibManager.sol            MIT
│   ├── IMessagingChannel.sol             MIT
│   ├── IMessagingComposer.sol            MIT
│   └── IMessagingContext.sol             MIT
├── libs/
│   ├── CalldataBytesLib.sol              LZBL-1.2
│   └── Transfer.sol                      LZBL-1.2  (imports OZ IERC20+SafeERC20)
└── messagelib/libs/BitMaps.sol           MIT

oapp-evm/contracts/oapp/
├── OApp.sol                              MIT
├── OAppCore.sol                          MIT       (imports OZ Ownable)
├── OAppReceiver.sol                      MIT
├── OAppSender.sol                        MIT       (imports OZ SafeERC20)
├── interfaces/
│   ├── IOAppCore.sol                     MIT
│   └── IOAppReceiver.sol                 MIT
└── libs/OptionsBuilder.sol               MIT       (imports OZ SafeCast + solidity-bytes-utils)
```

License distribution: 8 LZBL-1.2, 15 MIT, 1 dual MIT/Apache-2.0.

OpenZeppelin and `solidity-bytes-utils` imports remain remapped to their existing
top-level npm installs (OZ 5.4 / `solidity-bytes-utils` 0.8). Those don't need
vendoring.

## Source modifications

None. Files are byte-for-byte copies. Internal `./relative/path.sol` imports keep
working as-is; cross-package `@layerzerolabs/...` imports resolve through the
remappings in `contracts/remappings.txt`:

```
@layerzerolabs/oapp-evm/=vendor/layerzero/oapp-evm/
@layerzerolabs/lz-evm-protocol-v2/=vendor/layerzero/protocol-v2/
@layerzerolabs/lz-evm-messagelib-v2/=vendor/layerzero/messagelib-v2/
```

## Replaced upstream packages

- `@layerzerolabs/lz-evm-messagelib-v2` (3.0.x)
- `@layerzerolabs/lz-evm-protocol-v2` (3.0.x)
- `@layerzerolabs/oapp-evm` (0.4.x)
- `@layerzerolabs/lz-evm-v1-0.7` (was unused, dropped outright)
- `@layerzerolabs/test-devtools-evm-foundry` (replaced by `src/layerzero/mocks/SlimLayerZeroEndpoint.sol`)

## Refresh procedure

Manual — no quarterly drift CI yet. To bump:

1. `npm view @layerzerolabs/lz-evm-messagelib-v2 version` etc. for current versions.
2. `npm pack @layerzerolabs/lz-evm-messagelib-v2@<v>` and extract.
3. Diff each vendored file against the new tarball. Apply only the changed bytes —
   keep the import shape and the file list unchanged.
4. For `oapp-evm`, also bump the public commit hash in this file by checking the
   tarball's `gitHead` field via `npm view ... gitHead` against
   `https://github.com/LayerZero-Labs/devtools`.
5. Run `forge build && forge test` and update tarball SHA-512 + version in this
   file.
6. If a transitive import surfaces a new file (e.g. messagelib starts using a
   new internal lib), add it to `vendor/layerzero/...` rather than re-adding the
   npm package.
