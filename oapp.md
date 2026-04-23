# OApp Plan

## Decision

The LayerZero template ships a full starter stack by default:

- provider infra is deployed
- a starter OApp is deployed and wired
- `make e2e` remains part of the main LayerZero workflow

The starter OApp is template-managed application state, not required provider infrastructure.

## Chosen Shape

### Starter contract

- `TestOApp` is renamed to `ExampleOApp`
- it is starter code users can adapt or replace

### Environment config

Use a LayerZero-scoped toggle:

```json
{
  "layerzero": {
    "oapp": {
      "enabled": true
    }
  }
}
```

- shipped LayerZero envs set this explicitly to `true`
- provider-only mode is supported with `false`

### Deployment state

Store starter OApp addresses under:

```json
{
  "layerzero": {
    "oapp": {
      "source": "0x...",
      "destination": "0x..."
    }
  }
}
```

Provider infra stays under:

- `source.dvn`
- `destination.dvn`
- `destination.relayInfra.*`

## Behavior

### Deploy

- always deploy provider infra for LayerZero
- deploy and wire `ExampleOApp` when `layerzero.oapp.enabled` is `true`
- skip it when `false`

### Validate

- validate provider infra independently from starter OApp presence
- if `layerzero.oapp.enabled` is `false`, show a non-fatal note that `make send` and `make e2e` are unavailable

### Messaging commands

- `make send` and `make e2e` require starter OApp deployments
- if disabled or missing, fail with a clear error

## Migration

- switch directly from top-level `source.testOApp` / `destination.testOApp`
- no compatibility shim for the old deployment keys

## Non-goals

Not in this phase:

- arbitrary user-selected OApp contract names from config
- generic multi-app management
- compatibility with the legacy deployment schema
