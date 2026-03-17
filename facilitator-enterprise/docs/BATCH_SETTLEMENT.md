# Per-Network Batch Settlement Configuration

This document explains how to configure batch settlement parameters independently for each blockchain network.

## Overview

The enterprise facilitator supports per-network configuration for batch settlement, allowing you to tune batching parameters based on each network's characteristics:
- Block times (faster networks can flush more frequently)
- Gas costs (expensive networks benefit from larger batches)
- Traffic patterns (high-volume networks need different tuning)
- Risk tolerance (production vs testnet settings)

**Important:** Batch settlement currently supports **EIP-3009 exact** scheme only. Requests using Permit2, upto, or EIP-6492 wrapped signatures automatically fall back to direct settlement via the upstream facilitator. This fallback is seamless -- callers receive the same response format regardless of settlement path.

## Configuration Structure

Batch settlement configuration has two levels:
1. **Global defaults** - Applied to all networks
2. **Per-network overrides** - Override specific parameters for individual networks

All batch settlement configuration lives in `config.toml` (the enterprise TOML config).

### Global Configuration

```toml
[batch_settlement]
enabled = true                    # Enable batching globally
max_batch_size = 150             # Default max settlements per batch
max_wait_ms = 500                # Default max wait time (milliseconds)
min_batch_size = 10              # Default min size for immediate flush
allow_partial_failure = false    # Default failure mode (strict)
allow_hook_failure = false       # Allow hook failures without reverting batch
```

### Per-Network Overrides

```toml
[batch_settlement.networks.<network-name>]
enabled = <value>                # Optional: override enabled state
max_batch_size = <value>         # Optional: override max batch size
max_wait_ms = <value>            # Optional: override max wait time
min_batch_size = <value>         # Optional: override min batch size
allow_partial_failure = <value>  # Optional: override failure mode
```

**Network names** use the human-readable form (e.g., `base`, `base-sepolia`, `bsc`), not CAIP-2 chain IDs. The facilitator maps CAIP-2 identifiers like `eip155:8453` to `base` internally.

### Per-Network Enabled Override

You can enable batching for specific networks while leaving it globally disabled:

```toml
[batch_settlement]
enabled = false    # Globally disabled

[batch_settlement.networks.bsc-testnet]
enabled = true     # But enabled for BSC testnet only
max_batch_size = 100
```

## Example Configurations

### Example 1: Optimize for Different Networks

```toml
[batch_settlement]
enabled = true
max_batch_size = 150
max_wait_ms = 500
min_batch_size = 10
allow_partial_failure = false

# BSC: High throughput with larger batches
[batch_settlement.networks.bsc]
max_batch_size = 200
max_wait_ms = 1000
allow_partial_failure = true

# Base: Low latency for better UX
[batch_settlement.networks.base]
max_batch_size = 50
max_wait_ms = 250
min_batch_size = 5

# Avalanche: Aggressive batching for high throughput
[batch_settlement.networks.avalanche]
max_batch_size = 300
max_wait_ms = 2000
allow_partial_failure = true
```

### Example 2: Production vs Testnet

```toml
[batch_settlement]
enabled = true
max_batch_size = 150
max_wait_ms = 500
min_batch_size = 10
allow_partial_failure = false  # Strict for production

# Testnets: Faster flushing, permissive failure mode
[batch_settlement.networks.base-sepolia]
max_wait_ms = 250
allow_partial_failure = true

[batch_settlement.networks.bsc-testnet]
max_wait_ms = 250
allow_partial_failure = true

[batch_settlement.networks.avalanche-fuji]
max_wait_ms = 250
allow_partial_failure = true
```

### Example 3: Override Only What You Need

Each network can override any subset of parameters. Unspecified parameters use global defaults:

```toml
[batch_settlement]
enabled = true
max_batch_size = 150
max_wait_ms = 500
min_batch_size = 10
allow_partial_failure = false

# Only override batch size for BSC
[batch_settlement.networks.bsc]
max_batch_size = 200

# Only override wait time for Base
[batch_settlement.networks.base]
max_wait_ms = 250
```

## Parameter Guidelines

### max_batch_size
- **Range**: 1-545 (theoretical max based on gas limit)
- **Recommended**: 50-200 for safety
- **Consider**:
  - Gas limits (~30M / ~55k per transfer)
  - Network congestion
  - Risk tolerance

### max_wait_ms
- **Range**: 100-5000 milliseconds
- **Recommended**: 250-1000ms
- **Trade-offs**:
  - Lower = better latency, fewer batches
  - Higher = better throughput, more batching efficiency

### min_batch_size
- **Range**: 1-100
- **Recommended**: 5-20
- **Purpose**: Avoid waiting when enough settlements are ready

### allow_partial_failure
- **Values**: `true` or `false`
- **Default**: `false` (safer)
- **When true**: Individual transfers can fail without reverting entire batch
- **When false**: Any failure reverts all transfers (safer but less throughput)

### allow_hook_failure
- **Values**: `true` or `false`
- **Default**: `false` (safer)
- **When true**: Individual hook calls can fail without reverting the batch
- **When false**: Any hook failure reverts the entire batch including transfers

## How It Works

1. **Request Arrives**: A `/settle` request arrives at the `BatchFacilitator`.
2. **Scheme Check**: The facilitator checks if the request uses EIP-3009 exact scheme. Permit2, upto, and EIP-6492 requests skip batching and go directly to the upstream facilitator.
3. **Verification**: The request is verified via the upstream facilitator before being queued.
4. **Queue Creation**: When the first settlement arrives for a (facilitator, network) pair, a queue is created using that network's resolved configuration.
5. **Resolution**: The system looks up the network name (e.g., "bsc") and applies any overrides on top of global defaults.
6. **Batch Flush**: The queue flushes when `max_batch_size` is reached, `max_wait_ms` elapses, or `min_batch_size` is reached.
7. **Multicall3**: The batch is submitted as a single Multicall3 `aggregate3` transaction. If hooks are configured for any recipient, they are included as additional Call3 structs in the same transaction.
8. **Result Broadcast**: Each queued settlement receives the transaction result via a oneshot channel.

### Logging

The resolved configuration is logged when each queue is created:

```
INFO creating new batch queue for facilitator+network pair
  facilitator_addr=0x1234...
  network=bsc
  max_batch_size=200
  max_wait_ms=1000
  min_batch_size=10
  allow_partial_failure=true
```

## Monitoring

Check batch queue statistics via the admin endpoint:

```bash
curl -H "X-Admin-Key: your-admin-key" http://localhost:8080/admin/stats
```

Response includes:
```json
{
  "abuse": {
    "total_ips_tracked": 42,
    "suspicious_ips": 0
  },
  "batch": {
    "enabled": true,
    "active_queues": 3
  }
}
```

## Implementation Details

- **File**: `src/enterprise_config.rs` - Configuration structures and resolution logic
- **File**: `src/batch/queue.rs` - Queue creation with per-network config
- **File**: `src/batch/facilitator.rs` - Scheme detection and batch/direct routing
- **File**: `src/batch/processor.rs` - Multicall3 batch building and submission
- **Resolution Method**: `BatchSettlementConfig::for_network(network_name)` returns `ResolvedBatchConfig`

## Testing

Run the configuration tests:
```bash
cargo test -p facilitator-enterprise enterprise_config::tests
```

Tests verify:
- Global defaults
- Per-network overrides
- Partial overrides (mixing global and network-specific)
- Complete overrides (all parameters)
- Per-network enabled state override

## Best Practices

1. **Start Conservative**: Use global defaults and only override when needed
2. **Monitor Throughput**: Check queue statistics to see if batching is working
3. **Test on Testnets**: Experiment with aggressive settings on testnets first
4. **Consider Block Times**: Faster networks (Base: 2s) can use shorter wait times than slower networks
5. **Balance Latency vs Throughput**: Lower wait times = better UX, higher wait times = more efficient batching
6. **Use Partial Failure Carefully**: Only enable on networks/scenarios where you can tolerate individual failures

## Migration from Global Config

If you have existing global configuration:
```toml
[batch_settlement]
enabled = true
max_batch_size = 150
```

It continues to work exactly as before. Networks without overrides use these global defaults.

To add network-specific tuning, simply add network sections:
```toml
[batch_settlement]
enabled = true
max_batch_size = 150

# Add overrides without changing existing behavior for other networks
[batch_settlement.networks.bsc]
max_batch_size = 200
```

## Further Reading

- [Configuration Guide](CONFIGURATION.md) - Full configuration reference
- [Hooks Implementation Guide](HOOKS_IMPLEMENTATION.md) - Post-settlement hooks (executed within batch transactions)
- [API Reference](API.md) - Admin stats endpoint
- [Deployment Guide](DEPLOYMENT.md) - Production deployment
