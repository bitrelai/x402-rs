# Quick Start Guide

Get the enterprise facilitator up and running in minutes.

## Prerequisites

- Rust 1.80+ and Cargo
- RPC endpoints for your target blockchain networks
- Private key(s) for settlement transactions

## Installation

### Option 1: Build from Source

```bash
git clone https://github.com/x402-rs/x402-rs.git
cd x402-rs
cargo build --package facilitator-enterprise --features full --release
```

The binary is at `./target/release/facilitator-enterprise`.

### Option 2: Docker

```bash
docker build -f facilitator-enterprise/Dockerfile -t facilitator-enterprise .
```

## Basic Configuration

The enterprise facilitator uses a dual-config approach:

| File | Format | Purpose |
|------|--------|---------|
| `config.json` | JSON | Upstream chain/scheme/RPC configuration |
| `config.toml` | TOML | Enterprise security, rate limiting, batch settlement |
| `.env` | dotenv | API keys, admin key, file paths |

### 1. Create the Upstream Chain Config (JSON)

Copy the example and edit it with your RPC endpoints and signer wallets:

```bash
cp config.json.example config.json
```

Example `config.json`:

```json
{
  "port": 8080,
  "host": "0.0.0.0",
  "chains": {
    "eip155:84532": {
      "_comment": "Base Sepolia",
      "eip1559": true,
      "signers": ["0xYourPrivateKeyHere"],
      "rpc": [
        {
          "http": "https://sepolia.base.org",
          "rate_limit": 50
        }
      ]
    }
  },
  "schemes": [
    { "id": "v1-eip155-exact", "chains": "eip155:*" },
    { "id": "v2-eip155-exact", "chains": "eip155:*" }
  ]
}
```

The `CONFIG` environment variable controls where this file is loaded from (defaults to `./config.json`).

### 2. Create the Enterprise Config (TOML)

Copy the example:

```bash
cp facilitator-enterprise/config.toml.example facilitator-enterprise/config.toml
```

Edit `config.toml` for basic security (or leave defaults):

```toml
[rate_limiting]
enabled = true
requests_per_second = 10

[cors]
allowed_origins = []  # Empty = allow all (suitable for testing)

[security]
log_security_events = true
```

The `CONFIG_FILE` environment variable controls where this file is loaded from (defaults to `./config.toml`).

### 3. Create Environment File

Create a `.env` file:

```bash
HOST=0.0.0.0
PORT=8080

# Optional: Security
API_KEYS=your-secret-api-key-here
ADMIN_API_KEY=your-admin-key-here

# Optional: Logging
RUST_LOG=info
```

### 4. Run the Facilitator

```bash
cargo run --package facilitator-enterprise --features full --release
```

Or if already built:

```bash
./target/release/facilitator-enterprise
```

## Verify It's Running

### Check Health Endpoint

```bash
curl http://localhost:8080/health
```

Expected response:

```json
{
  "kinds": [
    {
      "version": "1.0",
      "scheme": "ERC-3009-TransferWithAuthorization",
      "network": "base-sepolia"
    }
  ]
}
```

### Check Root Endpoint

```bash
curl http://localhost:8080/
```

You should see an HTML page listing all available protocol and admin endpoints.

## Test a Payment Verification

Create a test payload file `test-verify.json`:

```json
{
  "paymentPayload": {
    "version": "1.0",
    "scheme": "ERC-3009-TransferWithAuthorization",
    "network": "base-sepolia",
    "from": "0xYourWalletAddress",
    "to": "0xFacilitatorAddress",
    "value": "1000000",
    "validAfter": "0",
    "validBefore": "999999999999",
    "nonce": "0x0000000000000000000000000000000000000000000000000000000000000001",
    "signature": "0xYourSignatureHere"
  },
  "paymentRequirements": {
    "version": "1.0",
    "scheme": "ERC-3009-TransferWithAuthorization",
    "network": "base-sepolia",
    "to": "0xFacilitatorAddress",
    "value": "1000000"
  }
}
```

Test verification:

```bash
curl -X POST http://localhost:8080/verify \
  -H "Authorization: Bearer your-api-key-here" \
  -H "Content-Type: application/json" \
  -d @test-verify.json
```

## Next Steps

- **Security Setup**: See [Configuration Guide](CONFIGURATION.md) for production security settings
- **Batch Settlement**: See [Batch Settlement Guide](BATCH_SETTLEMENT.md) for high-throughput configuration
- **Hooks**: See [Hooks Implementation Guide](HOOKS_IMPLEMENTATION.md) for post-settlement callbacks
- **Deployment**: See [Deployment Guide](DEPLOYMENT.md) for production deployment

## Common Issues

### "Failed to create Ethereum providers"

**Cause**: RPC URL not configured or unreachable in `config.json`.

**Solution**: Check your RPC configuration in `config.json` and ensure the endpoints are accessible:

```bash
curl -X POST https://sepolia.base.org \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}'
```

### "Invalid signature" errors

**Cause**: Signature doesn't match the payload or wrong signer.

**Solution**: Ensure the payment payload signature was created correctly with EIP-712 and the signer address matches the `from` field.

### Port already in use

**Cause**: Another process is using port 8080.

**Solution**: Change the `port` in your `config.json` or set `PORT` in your `.env` file:

```bash
PORT=8081
```

### "Config file not found"

**Cause**: The enterprise TOML config file is missing.

**Solution**: This is fine -- the facilitator uses sensible defaults. If you want custom security settings, create a `config.toml` file. See [Configuration Guide](CONFIGURATION.md).

## Getting Help

- **API Reference**: See [API Documentation](API.md)
- **Configuration**: See [Configuration Guide](CONFIGURATION.md)
- **Security Issues**: See [Security Documentation](SECURITY.md)
- **Upstream Protocol**: See [x402 Protocol Documentation](https://x402.org)
