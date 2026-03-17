# facilitator-enterprise

> Enterprise x402 facilitator binary with security middleware, post-settlement hooks, token filtering, and Multicall3 batch settlement.

## What This Is

`facilitator-enterprise` wraps the upstream [x402-rs](https://github.com/x402-rs/x402-rs) facilitator with production-hardening features originally developed in the [infra402-facilitator](https://github.com/infra402/infra402-facilitator) project:

- **Security middleware** -- rate limiting, IP filtering, API key auth, abuse detection
- **Post-settlement hooks** -- atomic on-chain callbacks via Multicall3 after each settlement
- **Batch settlement** -- bundle multiple EIP-3009 settlements into single Multicall3 transactions
- **Token filtering** -- restrict which tokens and networks the facilitator accepts
- **Admin API** -- runtime stats, hook management, and configuration reload

The upstream protocol logic (verification, settlement, scheme handling) is untouched. Enterprise features layer on top as Axum middleware and a `BatchFacilitator` wrapper.

## Architecture

```
                         facilitator-enterprise (this binary)
                        +--------------------------------------+
                        |  Security Middleware (TOML)          |
  HTTP request  ------> |    IP filter -> Rate limit -> Auth   |
                        |  BatchFacilitator                    |
                        |    EIP-3009 exact -> batch queue     |
                        |    Other schemes  -> direct settle   |
                        |  Upstream FacilitatorLocal (JSON)    |
                        |    Verify / Settle / Supported       |
                        +--------------------------------------+
                                        |
                          Multicall3 batch tx  OR  direct tx
                                        |
                                   Blockchain
```

## Configuration Files

| File | Format | Purpose |
|------|--------|---------|
| `config.json` | JSON | Upstream chain/scheme/RPC configuration (loaded by `x402-types`) |
| `config.toml` | TOML | Enterprise security, rate limiting, batch settlement |
| `hooks.toml` | TOML | Post-settlement hook definitions and per-network mappings |
| `tokens.toml` | TOML | Token filtering rules |
| `.env` | dotenv | API keys, admin key, file paths, OpenTelemetry |

See [docs/CONFIGURATION.md](docs/CONFIGURATION.md) for the complete reference.

## Quick Start

### 1. Set up configuration files

```bash
# Copy examples
cp config.json.example config.json   # upstream chain/scheme config
cp config.toml.example config.toml   # enterprise security config
cp .env.example .env
```

Edit `config.json` with your RPC endpoints and signer wallets. Edit `.env` with your API keys.

### 2. Build and run

```bash
# From the workspace root
cargo run --package facilitator-enterprise --features full --release
```

### 3. Verify it works

```bash
curl http://localhost:8080/health
curl http://localhost:8080/supported
```

### Docker

```bash
# Build from workspace root
docker build -f facilitator-enterprise/Dockerfile -t facilitator-enterprise .

# Run
docker run --env-file facilitator-enterprise/.env \
  -v $(pwd)/config.json:/app/config.json:ro \
  -v $(pwd)/facilitator-enterprise/config.toml:/app/config.toml:ro \
  -p 8080:8080 facilitator-enterprise
```

## Enterprise Features

### Security Middleware

Rate limiting, IP allow/block lists, API key authentication, abuse detection, CORS control, and request size limits. All configured via `config.toml`.

See [docs/SECURITY.md](docs/SECURITY.md).

### Post-Settlement Hooks

Execute custom smart contract calls atomically with each settlement via Multicall3. Supports parameterized calldata built from payment data, runtime context, and static values.

See [docs/HOOKS_IMPLEMENTATION.md](docs/HOOKS_IMPLEMENTATION.md).

### Batch Settlement

Bundle multiple EIP-3009 `transferWithAuthorization` calls into single Multicall3 transactions. Per-network tuning for batch size, wait time, and failure mode.

**Note:** Batch settlement currently supports **EIP-3009 exact** scheme only. Permit2, upto, and EIP-6492 requests automatically fall back to direct settlement.

See [docs/BATCH_SETTLEMENT.md](docs/BATCH_SETTLEMENT.md).

### Admin API

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/admin/stats` | GET | Security and batch queue statistics |
| `/admin/hooks` | GET | List hook definitions |
| `/admin/hooks/mappings` | GET | List destination mappings |
| `/admin/hooks/status` | GET | Hook system status |
| `/admin/hooks/reload` | POST | Reload hooks.toml |
| `/admin/hooks/{name}/enable` | POST | Enable a hook |
| `/admin/hooks/{name}/disable` | POST | Disable a hook |
| `/admin/tokens/reload` | POST | Reload tokens.toml |

All admin endpoints require the `X-Admin-Key` header.

See [docs/API.md](docs/API.md).

## Documentation

| Document | Description |
|----------|-------------|
| [Quick Start](docs/QUICK_START.md) | Get running in minutes |
| [Configuration](docs/CONFIGURATION.md) | Complete config reference (JSON + TOML + env) |
| [API Reference](docs/API.md) | HTTP endpoints, auth, and error responses |
| [Security](docs/SECURITY.md) | Security features and production hardening |
| [Batch Settlement](docs/BATCH_SETTLEMENT.md) | High-throughput batch configuration |
| [Hooks](docs/HOOKS_IMPLEMENTATION.md) | Post-settlement hook system |
| [Deployment](docs/DEPLOYMENT.md) | Docker, Kubernetes, reverse proxy, systemd |

## Acknowledgements

This crate builds on two upstream projects:

- **[x402-rs/x402-rs](https://github.com/x402-rs/x402-rs)** -- Core x402 protocol implementation, facilitator traits, chain providers, and scheme handlers. All protocol logic comes from here.
- **[infra402/infra402-facilitator](https://github.com/infra402/infra402-facilitator)** -- Enterprise features (security middleware, hooks, batch settlement, token filtering) were originally developed here as a monolithic fork and have been extracted into this modular crate.

## License

Apache-2.0 -- see [LICENSE](../LICENSE) for details.
