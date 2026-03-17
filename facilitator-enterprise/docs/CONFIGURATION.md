# Configuration Guide

Complete reference for configuring the enterprise facilitator. The facilitator uses a dual-config approach: upstream JSON for chain/scheme setup, and enterprise TOML for security, batch settlement, and operational settings.

## Configuration Overview

| File | Format | Loaded By | Purpose |
|------|--------|-----------|---------|
| `config.json` | JSON | `x402-types` (upstream) | Chain providers, RPC endpoints, signers, scheme registration |
| `config.toml` | TOML | `EnterpriseConfig` | Rate limiting, CORS, IP filtering, batch settlement, security |
| `hooks.toml` | TOML | `HookManager` | Post-settlement hook definitions and per-network mappings |
| `tokens.toml` | TOML | `TokenManager` | Token filtering rules |
| `.env` | dotenv | `dotenvy` | API keys, admin key, file paths, OpenTelemetry |

## Upstream Chain Config (config.json)

The upstream chain/scheme configuration uses JSON format, matching the standard `x402-rs` facilitator. This file defines which blockchain networks and payment schemes the facilitator supports.

### File Location

Set via the `CONFIG` environment variable (defaults to `./config.json`):

```bash
CONFIG=/path/to/config.json facilitator-enterprise
```

### Structure

```json
{
  "port": 8080,
  "host": "0.0.0.0",
  "chains": {
    "eip155:<chain-id>": {
      "eip1559": true,
      "signers": ["0xPrivateKey1", "0xPrivateKey2"],
      "rpc": [
        {
          "http": "https://rpc.example.com",
          "rate_limit": 50
        }
      ]
    },
    "solana:<reference>": {
      "signer": "Base58PrivateKey",
      "rpc": "https://rpc.example.com",
      "pubsub": "wss://rpc.example.com"
    }
  },
  "schemes": [
    { "id": "v1-eip155-exact", "chains": "eip155:*" },
    { "id": "v2-eip155-exact", "chains": "eip155:*" },
    { "id": "v2-eip155-upto", "chains": "eip155:*" },
    { "id": "v1-solana-exact", "chains": "solana:*" },
    { "id": "v2-solana-exact", "chains": "solana:*" }
  ]
}
```

### Key Points

- **Signers**: EVM chains accept an array of private keys for multi-wallet round-robin settlement.
- **RPC**: EVM chains accept an array of RPC endpoints with optional rate limits.
- **Schemes**: Use wildcard patterns (`eip155:*`) to register schemes across all chains in a namespace.
- **Networks**: Only chains listed here are available for verification and settlement. The `/supported` endpoint reflects exactly what you configure.

For the full upstream configuration reference, see the [x402-rs documentation](https://github.com/x402-rs/x402-rs).

## Enterprise Config (config.toml)

The enterprise TOML configuration controls security, rate limiting, batch settlement, and operational settings. All sections are optional and have sensible defaults.

### File Location

Set via the `CONFIG_FILE` environment variable (defaults to `./config.toml`):

```bash
CONFIG_FILE=/path/to/config.toml facilitator-enterprise
```

If the file does not exist, the facilitator uses defaults and logs a message.

### Configuration Structure

```toml
[rate_limiting]
# Rate limiting settings

[cors]
# CORS settings

[ip_filtering]
# IP allow/block lists

[request]
# Request validation settings

[security]
# Security and logging settings

[transaction]
# Chain-specific transaction settings

[batch_settlement]
# Batch settlement settings
```

## Rate Limiting Configuration

### Basic Rate Limiting

```toml
[rate_limiting]
enabled = true
requests_per_second = 10          # Global limit per IP
ban_duration_seconds = 300         # 5 minutes
ban_threshold = 5                  # Violations before ban
# whitelisted_ips = ["127.0.0.0/8", "::1/128"]
```

### Parameters

- **`enabled`** (boolean): Enable/disable rate limiting globally
  - Default: `true`

- **`requests_per_second`** (integer): Maximum requests per second per IP address
  - Default: `10`
  - Applies to all endpoints unless overridden

- **`ban_duration_seconds`** (integer): Duration in seconds to ban an IP after exceeding `ban_threshold`
  - Default: `300` (5 minutes)

- **`ban_threshold`** (integer): Number of rate limit violations before triggering a ban
  - Default: `5`

- **`whitelisted_ips`** (array of strings): IP addresses or CIDR blocks exempt from rate limiting
  - Default: `["127.0.0.0/8", "::1/128", "::ffff:127.0.0.0/104"]` (localhost)

### Per-Endpoint Overrides

Override rate limits for specific endpoints:

```toml
[rate_limiting.endpoints]
verify = 25    # 25 requests/second for /verify
settle = 25    # 25 requests/second for /settle
health = 100   # 100 requests/second for /health
```

Endpoint names should match route paths without the leading slash.

### Behavior

1. **Normal Operation**: Requests within limit are processed normally
2. **Rate Limit Exceeded**: Returns `429 Too Many Requests` with `Retry-After` header
3. **Ban Trigger**: After `ban_threshold` violations, IP is banned for `ban_duration_seconds`
4. **Ban Active**: Returns `403 Forbidden` for all requests during ban period
5. **Ban Expiry**: Ban automatically expires and IP can retry

## CORS Configuration

### Allow All Origins (Development)

```toml
[cors]
allowed_origins = []
```

Empty list allows all origins (`*`).

### Restrict Origins (Production)

```toml
[cors]
allowed_origins = [
    "https://app.example.com",
    "https://dashboard.example.com",
]
```

Only specified origins will be permitted for cross-origin requests.

### Parameters

- **`allowed_origins`** (array of strings): List of allowed origin URLs
  - Default: `[]` (allow all)
  - Format: Full URL including protocol (e.g., `https://example.com`)

## IP Filtering Configuration

### Allow/Block Lists

```toml
[ip_filtering]
allowed_ips = [
    "192.168.1.0/24",    # Internal network CIDR
    "10.0.0.1",          # Specific IP
    "2001:db8::/32",     # IPv6 CIDR
]

blocked_ips = [
    "192.0.2.0/24",      # Known malicious range
    "198.51.100.50",     # Specific blocked IP
]
```

### Parameters

- **`allowed_ips`** (array of strings): Allowed IP addresses or CIDR blocks
  - Default: `[]` (allow all)
  - If specified, **only** these IPs/ranges will be allowed
  - Supports IPv4, IPv6, and CIDR notation

- **`blocked_ips`** (array of strings): Blocked IP addresses or CIDR blocks
  - Default: `[]`
  - IPs on this list are **always** rejected, regardless of allow list
  - Supports IPv4, IPv6, and CIDR notation

### Precedence

1. **Blocked list** is checked first
2. If IP is in `blocked_ips`, request is rejected (`403 Forbidden`)
3. If `allowed_ips` is non-empty and IP is **not** in the list, request is rejected
4. Otherwise, request proceeds to next middleware

## Request Configuration

### Body Size Limits

```toml
[request]
max_body_size_bytes = 1048576  # 1 MB
```

### Parameters

- **`max_body_size_bytes`** (integer): Maximum size of HTTP request body in bytes
  - Default: `1048576` (1 MB)
  - Requests exceeding this size return `413 Payload Too Large`

## Security Configuration

### General Security Settings

```toml
[security]
health_endpoint_requires_auth = false
log_security_events = true
cleanup_interval_seconds = 300
```

### Parameters

- **`health_endpoint_requires_auth`** (boolean): Require API key for `/health` endpoint
  - Default: `false` (public access)

- **`log_security_events`** (boolean): Enable logging of security-related events
  - Default: `true`
  - Logs: rate limit violations, auth failures, blocked IPs, suspicious activity

- **`cleanup_interval_seconds`** (integer): Interval in seconds for background cleanup of tracking data
  - Default: `300` (5 minutes)
  - Cleans up: old abuse detection data, expired rate limit bans

### API Key Authentication

Configured via environment variables (not `config.toml`):

```bash
# Enable API key auth for /verify and /settle
export API_KEYS="key1,key2,key3"

# Enable admin key auth for /admin/*
export ADMIN_API_KEY="your-admin-secret"
```

**API Keys**:
- Comma-separated list
- Clients use: `Authorization: Bearer <key>`
- Applied to: `/verify`, `/settle`

**Admin Key**:
- Single secret key
- Clients use: `X-Admin-Key: <key>`
- Applied to: `/admin/*`

## Transaction Configuration

### Default Timeout

```toml
[transaction]
default_rpc_timeout_seconds = 30
```

### Per-Chain Configuration

Configure block times and timeouts for each blockchain network:

```toml
[transaction.chains.bsc]
block_time_seconds = 3
receipt_timeout_blocks = 20        # 60s total (20 * 3s)
rpc_request_timeout_seconds = 15

[transaction.chains.base]
block_time_seconds = 2
receipt_timeout_blocks = 30        # 60s total (30 * 2s)
rpc_request_timeout_seconds = 20
```

### Parameters

- **`default_rpc_timeout_seconds`** (integer): Fallback RPC timeout when chain-specific config is missing
  - Default: `30`

#### Per-Chain Parameters

- **`block_time_seconds`** (integer): Average block time for this chain
- **`receipt_timeout_blocks`** (integer): Number of blocks to wait for transaction receipt
  - Total timeout = `block_time_seconds * receipt_timeout_blocks`
- **`rpc_request_timeout_seconds`** (integer): Timeout for individual RPC requests

## Batch Settlement Configuration

See [Batch Settlement Guide](BATCH_SETTLEMENT.md) for the complete configuration reference.

### Quick Example

```toml
[batch_settlement]
enabled = true
max_batch_size = 150
max_wait_ms = 500
min_batch_size = 10
allow_partial_failure = false
allow_hook_failure = false

# Per-network overrides
[batch_settlement.networks.bsc]
max_batch_size = 200
max_wait_ms = 1000
```

**Note:** Batch settlement currently supports **EIP-3009 exact** scheme only. Permit2, upto, and EIP-6492 requests automatically fall back to direct settlement via the upstream facilitator.

## Hook Configuration

See [Hooks Implementation Guide](HOOKS_IMPLEMENTATION.md) for the complete configuration reference.

Hooks are configured in a separate `hooks.toml` file (path set via `HOOKS_FILE` env var, defaults to `./hooks.toml`).

## Token Configuration

Token filtering is configured in a separate `tokens.toml` file (path set via `TOKENS_FILE` env var, defaults to `./tokens.toml`).

## Environment Variables

All environment variables used by the enterprise facilitator:

### Server

```bash
HOST=0.0.0.0        # Bind address (default: 0.0.0.0)
PORT=8080           # HTTP port (default: 8080)
```

### Configuration File Paths

```bash
CONFIG=/path/to/config.json          # Upstream chain/scheme config (default: ./config.json)
CONFIG_FILE=/path/to/config.toml     # Enterprise security config (default: ./config.toml)
HOOKS_FILE=/path/to/hooks.toml       # Hook definitions (default: ./hooks.toml)
TOKENS_FILE=/path/to/tokens.toml     # Token filtering (default: ./tokens.toml)
```

### Security

```bash
API_KEYS=key1,key2,key3           # API key authentication
ADMIN_API_KEY=admin-secret         # Admin authentication
```

### Observability

```bash
RUST_LOG=info                                              # Log level
OTEL_EXPORTER_OTLP_ENDPOINT=https://api.honeycomb.io:443  # OpenTelemetry endpoint
OTEL_EXPORTER_OTLP_HEADERS=x-honeycomb-team=API_KEY       # OTLP headers
OTEL_EXPORTER_OTLP_PROTOCOL=http/protobuf                 # OTLP protocol
```

### Hook Contract Addresses

Hook contract addresses and recipient addresses are referenced from `hooks.toml` via `${ENV_VAR}` substitution:

```bash
# Example: contract addresses for hooks
HOOK_TOKEN_MINT_CALLBACK_BASE_SEPOLIA_CONTRACT=0x...
TOKEN_MINT_RECIPIENT_BASE_SEPOLIA=0x...
```

## Configuration Examples

### Development (Permissive)

**config.toml:**

```toml
[rate_limiting]
enabled = false

[cors]
allowed_origins = []

[ip_filtering]
allowed_ips = []
blocked_ips = []

[security]
log_security_events = true
```

### Production (Strict)

**config.toml:**

```toml
[rate_limiting]
enabled = true
requests_per_second = 50
ban_duration_seconds = 600
ban_threshold = 5

[rate_limiting.endpoints]
verify = 25
settle = 25

[cors]
allowed_origins = [
    "https://app.example.com",
    "https://dashboard.example.com",
]

[ip_filtering]
allowed_ips = []  # Or specify trusted IPs
blocked_ips = [
    # Add known malicious IPs/ranges
]

[request]
max_body_size_bytes = 1048576

[security]
health_endpoint_requires_auth = false
log_security_events = true
cleanup_interval_seconds = 3600
```

**.env:**

```bash
API_KEYS=prod-key-1,prod-key-2,prod-key-3
ADMIN_API_KEY=secure-admin-secret-here
RUST_LOG=info
```

## Reloading Configuration

Enterprise TOML configuration changes require a service restart:

```bash
# Send SIGTERM for graceful shutdown
kill -TERM $(pidof facilitator-enterprise)

# Restart
./facilitator-enterprise
```

Or with systemd:

```bash
sudo systemctl restart facilitator-enterprise
```

**Exception:** Hooks and tokens support hot-reload via the admin API without restarting:

```bash
# Reload hooks.toml
curl -X POST http://localhost:8080/admin/hooks/reload \
  -H "X-Admin-Key: your-admin-key"

# Reload tokens.toml
curl -X POST http://localhost:8080/admin/tokens/reload \
  -H "X-Admin-Key: your-admin-key"
```

## Validating Configuration

Test your configuration before deploying:

```bash
# Dry-run to check for parsing errors
cargo run --package facilitator-enterprise --features full --release 2>&1 | head -20
```

Check startup logs for confirmation:

```
INFO Configuration loaded successfully
INFO Rate limiting enabled: true
INFO CORS: Allowing all origins (*)
INFO Security: log_security_events=true
```

## Further Reading

- [Security Documentation](SECURITY.md) - Security best practices and hardening
- [API Reference](API.md) - API endpoints and authentication
- [Deployment Guide](DEPLOYMENT.md) - Production deployment checklist
- [Batch Settlement Guide](BATCH_SETTLEMENT.md) - High-throughput configuration
