# Post-Settlement Hooks Implementation Guide

This guide explains how to configure and use the post-settlement hook system in the enterprise facilitator.

## Overview

Post-settlement hooks allow you to execute custom contract calls atomically with settlement transfers via Multicall3. This enables additional on-chain logic to run immediately after a payment is settled, all within the same transaction for atomicity.

## Hook Types

### Parameterized Hooks

Dynamic calldata built from payment data, runtime context, or static values. Provides full access to EIP-3009 parameters and runtime information.

**Advantages:**
- Type-safe parameter resolution
- Access to full EIP-3009 authorization data
- Runtime context (timestamps, block numbers, batch info)
- Flexible parameter composition

## Configuration Structure

Hooks are configured in `hooks.toml` (path set via `HOOKS_FILE` env var, defaults to `./hooks.toml`):

```toml
[hooks]
enabled = true/false                    # Global enable/disable
allow_hook_failure = true/false        # Allow individual hooks to fail

# Hook definitions (shared across all networks)
[hooks.definitions.hook_name]
enabled = true
function_signature = "funcName(type1,type2,...)"
gas_limit = 100000
description = "What this hook does"

[[hooks.definitions.hook_name.parameters]]
type = "solidity_type"
source = { source_type = "payment|runtime|config|static", field = "..." }

# Per-network configuration
[hooks.networks.network-name]
enabled = true

# Recipient address -> hook names mapping (supports env vars)
[hooks.networks.network-name.mappings]
"${RECIPIENT_ENV_VAR}" = ["hook_name"]

# Hook name -> contract address mapping (supports env vars)
[hooks.networks.network-name.contracts]
hook_name = "${CONTRACT_ENV_VAR}"
```

**Environment Variables (.env):**
```bash
RECIPIENT_ENV_VAR=0xRecipientAddress
CONTRACT_ENV_VAR=0xContractAddress
```

## Available Parameter Sources

### Payment Fields (from EIP-3009 authorization)

Extract data from the EIP-3009 `transferWithAuthorization` parameters:

| Field | Type | Description |
|-------|------|-------------|
| `from` | address | Payer address |
| `to` | address | Recipient address |
| `value` | uint256 | Transfer amount |
| `validafter` | uint256 | Valid after timestamp |
| `validbefore` | uint256 | Valid before timestamp |
| `nonce` | bytes32 | Unique nonce |
| `contractaddress` | address | Token contract address |
| `signaturev` | uint8 | Signature v component |
| `signaturer` | bytes32 | Signature r component |
| `signatures` | bytes32 | Signature s component |

**Example:**
```toml
[[hooks.definitions.my_hook.parameters]]
type = "address"
source = { source_type = "payment", field = "from" }
```

### Runtime Fields (from settlement context)

Access dynamic values available at settlement time:

| Field | Type | Description |
|-------|------|-------------|
| `timestamp` | uint256 | Block timestamp (block.timestamp) |
| `blocknumber` | uint256 | Block number (block.number) |
| `sender` | address | Facilitator address (msg.sender) |
| `batchindex` | uint256 | Position in current batch (0-indexed) |
| `batchsize` | uint256 | Total settlements in batch |

**Example:**
```toml
[[hooks.definitions.my_hook.parameters]]
type = "uint256"
source = { source_type = "runtime", field = "timestamp" }
```

### Config Values

Use values from the hook's custom configuration:

**Example:**
```toml
[hooks.definitions.my_hook.config_values]
recipient = "0x1111111111111111111111111111111111111111"
percentage = "250"

[[hooks.definitions.my_hook.parameters]]
type = "address"
source = { source_type = "config", field = "recipient" }
```

### Static Values

Use literal values directly in the configuration:

**Example:**
```toml
[[hooks.definitions.my_hook.parameters]]
type = "uint256"
source = { source_type = "static", value = "100" }
```

## Example Implementations

### Example 1: TokenMintWith3009 EIP-3009 Callback (Complete Step-by-Step Guide)

This example shows how to set up a hook that calls `onPaymentReceived` on a TokenMintWith3009 contract whenever it receives a payment. The hook passes all EIP-3009 authorization parameters including signature components.

#### Overview

When a user sends a payment to your TokenMintWith3009 contract, the facilitator will:
1. Execute the EIP-3009 `transferWithAuthorization` on the token contract
2. Immediately call `onPaymentReceived` on your hook contract with all payment details
3. Both calls execute atomically in a single Multicall3 transaction

#### Step 1: Deploy Your Hook Contract

**1.1 Write the Solidity Contract**

Your contract must implement the `onPaymentReceived` function:

```solidity
// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

interface ITokenMintWith3009 {
    function onPaymentReceived(
        address from,        // Payer address
        uint256 value,       // Amount transferred
        uint256 validAfter,  // EIP-3009 validAfter timestamp
        uint256 validBefore, // EIP-3009 validBefore timestamp
        bytes32 nonce,       // EIP-3009 unique nonce
        uint8 v,             // Signature v component
        bytes32 r,           // Signature r component
        bytes32 s            // Signature s component
    ) external;
}

contract TokenMintWith3009 is ITokenMintWith3009 {
    event PaymentReceived(address indexed from, uint256 value, bytes32 nonce);

    function onPaymentReceived(
        address from,
        uint256 value,
        uint256 validAfter,
        uint256 validBefore,
        bytes32 nonce,
        uint8 v,
        bytes32 r,
        bytes32 s
    ) external override {
        // Your custom logic here
        // Example: Mint tokens, NFTs, update state, etc.

        emit PaymentReceived(from, value, nonce);
    }
}
```

**1.2 Compile and Deploy**

```bash
# Using Foundry
forge build
forge create --rpc-url https://sepolia.base.org \
  --private-key $PRIVATE_KEY \
  src/TokenMintWith3009.sol:TokenMintWith3009
```

**Note the deployed contract address** -- you will need it for configuration.

#### Step 2: Configure Hook in hooks.toml

**2.1 Add Hook Definition (Shared Across Networks)**

Edit `hooks.toml` and add your hook definition:

```toml
[hooks]
enabled = true
allow_hook_failure = false  # Recommended: hooks must succeed

# Hook definitions are shared across all networks
[hooks.definitions.token_mint_callback]
enabled = true
description = "Calls onPaymentReceived with EIP-3009 authorization parameters"
function_signature = "onPaymentReceived(address,uint256,uint256,uint256,bytes32,uint8,bytes32,bytes32)"
gas_limit = 200000  # Adjust based on your contract's gas usage

# Parameter 1: from (payer address)
[[hooks.definitions.token_mint_callback.parameters]]
type = "address"
source = { source_type = "payment", field = "from" }

# Parameter 2: value (transfer amount)
[[hooks.definitions.token_mint_callback.parameters]]
type = "uint256"
source = { source_type = "payment", field = "value" }

# Parameter 3: validAfter (EIP-3009 timestamp)
[[hooks.definitions.token_mint_callback.parameters]]
type = "uint256"
source = { source_type = "payment", field = "validafter" }

# Parameter 4: validBefore (EIP-3009 timestamp)
[[hooks.definitions.token_mint_callback.parameters]]
type = "uint256"
source = { source_type = "payment", field = "validbefore" }

# Parameter 5: nonce (EIP-3009 unique nonce)
[[hooks.definitions.token_mint_callback.parameters]]
type = "bytes32"
source = { source_type = "payment", field = "nonce" }

# Parameter 6: v (signature component)
[[hooks.definitions.token_mint_callback.parameters]]
type = "uint8"
source = { source_type = "payment", field = "signaturev" }

# Parameter 7: r (signature component)
[[hooks.definitions.token_mint_callback.parameters]]
type = "bytes32"
source = { source_type = "payment", field = "signaturer" }

# Parameter 8: s (signature component)
[[hooks.definitions.token_mint_callback.parameters]]
type = "bytes32"
source = { source_type = "payment", field = "signatures" }
```

**2.2 Configure Per-Network Settings**

Add network-specific configuration for Base Sepolia (testnet):

```toml
# Base Sepolia testnet configuration
[hooks.networks.base-sepolia]
enabled = true

# Recipient address -> hook names mapping for this network
# Supports environment variable substitution: "${ENV_VAR_NAME}"
[hooks.networks.base-sepolia.mappings]
"${TOKEN_MINT_RECIPIENT_BASE_SEPOLIA}" = ["token_mint_callback"]

# Hook name -> contract address mapping for this network
# Supports environment variable substitution: "${ENV_VAR_NAME}"
[hooks.networks.base-sepolia.contracts]
token_mint_callback = "${HOOK_TOKEN_MINT_CALLBACK_BASE_SEPOLIA_CONTRACT}"
```

**Important**: Both recipient addresses in mappings and contract addresses support environment variable substitution using `"${ENV_VAR_NAME}"` syntax. This keeps hooks.toml git-committable without exposing deployment addresses.

**2.3 Set Environment Variables**

Add to your `.env` file (not committed to git):

```bash
# Recipient address that will receive payments and trigger the hook
TOKEN_MINT_RECIPIENT_BASE_SEPOLIA=0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb

# Hook contract address where the callback function is implemented
HOOK_TOKEN_MINT_CALLBACK_BASE_SEPOLIA_CONTRACT=0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb
```

**Note**: In this example, the recipient and contract are the same address (the TokenMint contract receives the payment AND handles the callback). They can be different addresses depending on your use case.

**2.4 Production Configuration Example**

For mainnet, use environment variables to keep hooks.toml git-committable:

```toml
# Base mainnet configuration
[hooks.networks.base]
enabled = true

[hooks.networks.base.mappings]
"${TOKEN_MINT_RECIPIENT_BASE}" = ["token_mint_callback"]

[hooks.networks.base.contracts]
token_mint_callback = "${HOOK_TOKEN_MINT_CALLBACK_BASE_CONTRACT}"
```

Then set production addresses in `.env` (managed separately via CI/CD or secrets management):

```bash
TOKEN_MINT_RECIPIENT_BASE=0xYourProductionRecipientAddress
HOOK_TOKEN_MINT_CALLBACK_BASE_CONTRACT=0xYourProductionContractAddress
```

#### Step 3: Test the Hook

**3.1 Reload Configuration**

If the facilitator is running, reload the configuration:

```bash
curl -X POST http://localhost:8080/admin/hooks/reload \
  -H "X-Admin-Key: your-admin-key"
```

Or restart the facilitator:

```bash
cargo run --package facilitator-enterprise --features full
```

**3.2 Verify Hook is Loaded**

Check hook status:

```bash
curl http://localhost:8080/admin/hooks/status \
  -H "X-Admin-Key: your-admin-key"

# Should show: enabled: true, hooks_count: 1
```

**3.3 Send Test Payment**

Send a settlement to the mapped recipient address via the `/settle` endpoint.

**3.4 Verify Execution**

Check the transaction on the block explorer:

1. Find the settlement transaction hash from the API response
2. Open in Basescan (or your network's explorer)
3. Look for:
   - **Transfer event** from USDC contract (payment executed)
   - **PaymentReceived event** from your contract (hook executed)
4. Verify both events are in the same transaction

**Example transaction:**
```
Multicall3.aggregate3()
  +-- USDC.transferWithAuthorization()
  +-- TokenMintWith3009.onPaymentReceived()
```

#### Step 4: Troubleshooting

**Hook Not Executing**

1. **Check hook is enabled**:
   ```bash
   curl http://localhost:8080/admin/hooks \
     -H "X-Admin-Key: your-admin-key"
   ```

2. **Verify recipient mapping**:
   ```bash
   curl http://localhost:8080/admin/hooks/mappings \
     -H "X-Admin-Key: your-admin-key"
   ```

3. **Check facilitator logs**:
   ```bash
   # Look for hook resolution logs
   grep "Retrieved hooks for destination" logs/facilitator.log
   ```

**Function Signature Mismatch**

Verify your function signature matches the ABI:

```bash
# Extract function signature from ABI
cast sig "onPaymentReceived(address,uint256,uint256,uint256,bytes32,uint8,bytes32,bytes32)"

# Should match your hooks.toml function_signature
```

**Out of Gas**

If hook reverts with out-of-gas, increase `gas_limit` in hooks.toml with 20% buffer:

```toml
gas_limit = 240000  # If estimate was 200k
```

#### Integration Checklist

Before going to production, verify:

- [ ] Contract deployed and verified on block explorer
- [ ] Contract address configured in hooks.toml (via env var substitution)
- [ ] Function signature exactly matches contract ABI
- [ ] All parameters configured with correct types and sources
- [ ] Hook tested successfully on testnet
- [ ] Gas limit verified (estimate + 20% buffer)
- [ ] `allow_hook_failure = false` for production safety
- [ ] Transaction monitoring set up to catch hook failures
- [ ] Facilitator logs configured to capture hook execution

### Example 2: Simple Settlement Notification

Notifies a contract when a settlement occurs with basic information.

**Solidity Interface:**
```solidity
interface ISettlementNotifier {
    function notifySettlement(
        address from,
        address to,
        uint256 amount
    ) external;
}
```

**Configuration:**
```toml
# Hook definition (shared across networks)
[hooks.definitions.notify_settlement]
enabled = true
description = "Notifies contract of settlement with payer, recipient, and amount"
function_signature = "notifySettlement(address,address,uint256)"
gas_limit = 100000

[[hooks.definitions.notify_settlement.parameters]]
type = "address"
source = { source_type = "payment", field = "from" }

[[hooks.definitions.notify_settlement.parameters]]
type = "address"
source = { source_type = "payment", field = "to" }

[[hooks.definitions.notify_settlement.parameters]]
type = "uint256"
source = { source_type = "payment", field = "value" }

# Per-network configuration (e.g., Base Sepolia)
[hooks.networks.base-sepolia]
enabled = true

[hooks.networks.base-sepolia.mappings]
"${NOTIFY_RECIPIENT_BASE_SEPOLIA}" = ["notify_settlement"]

[hooks.networks.base-sepolia.contracts]
notify_settlement = "${NOTIFY_CONTRACT_BASE_SEPOLIA}"
```

**Environment Variables (.env):**
```bash
NOTIFY_RECIPIENT_BASE_SEPOLIA=0xRecipientAddress
NOTIFY_CONTRACT_BASE_SEPOLIA=0xNotifierContractAddress
```

### Example 3: Settlement with Runtime Context

Includes block timestamp and facilitator address.

**Solidity Interface:**
```solidity
interface ISettlementRecorder {
    function recordSettlement(
        address from,
        address to,
        uint256 amount,
        uint256 timestamp,
        address facilitator
    ) external;
}
```

**Configuration:**
```toml
# Hook definition (shared across networks)
[hooks.definitions.settlement_with_timestamp]
enabled = true
description = "Notifies contract with settlement details and block timestamp"
function_signature = "recordSettlement(address,address,uint256,uint256,address)"
gas_limit = 120000

[[hooks.definitions.settlement_with_timestamp.parameters]]
type = "address"
source = { source_type = "payment", field = "from" }

[[hooks.definitions.settlement_with_timestamp.parameters]]
type = "address"
source = { source_type = "payment", field = "to" }

[[hooks.definitions.settlement_with_timestamp.parameters]]
type = "uint256"
source = { source_type = "payment", field = "value" }

[[hooks.definitions.settlement_with_timestamp.parameters]]
type = "uint256"
source = { source_type = "runtime", field = "timestamp" }

[[hooks.definitions.settlement_with_timestamp.parameters]]
type = "address"
source = { source_type = "runtime", field = "sender" }

# Per-network configuration
[hooks.networks.base-sepolia]
enabled = true

[hooks.networks.base-sepolia.mappings]
"${RECORDER_RECIPIENT_BASE_SEPOLIA}" = ["settlement_with_timestamp"]

[hooks.networks.base-sepolia.contracts]
settlement_with_timestamp = "${RECORDER_CONTRACT_BASE_SEPOLIA}"
```

**Environment Variables (.env):**
```bash
RECORDER_RECIPIENT_BASE_SEPOLIA=0xRecipientAddress
RECORDER_CONTRACT_BASE_SEPOLIA=0xRecorderContractAddress
```

### Example 4: Mixed Static and Dynamic Parameters

Combines payment data with static configuration values for royalty distribution.

**Solidity Interface:**
```solidity
interface IRoyaltyDistributor {
    function distributeRoyalty(
        address payer,
        uint256 amount,
        address royaltyRecipient,
        uint256 royaltyPercentage
    ) external;
}
```

**Configuration:**
```toml
# Hook definition (shared across networks)
[hooks.definitions.royalty_distribution]
enabled = true
description = "Distributes royalties with configured percentages"
function_signature = "distributeRoyalty(address,uint256,address,uint256)"
gas_limit = 150000

[hooks.definitions.royalty_distribution.config_values]
royalty_recipient = "${ROYALTY_RECIPIENT_ADDRESS}"
royalty_percentage = "250"  # 2.5% in basis points

[[hooks.definitions.royalty_distribution.parameters]]
type = "address"
source = { source_type = "payment", field = "from" }

[[hooks.definitions.royalty_distribution.parameters]]
type = "uint256"
source = { source_type = "payment", field = "value" }

[[hooks.definitions.royalty_distribution.parameters]]
type = "address"
source = { source_type = "config", field = "royalty_recipient" }

[[hooks.definitions.royalty_distribution.parameters]]
type = "uint256"
source = { source_type = "config", field = "royalty_percentage" }

# Per-network configuration
[hooks.networks.base-sepolia]
enabled = true

[hooks.networks.base-sepolia.mappings]
"${ROYALTY_TRIGGER_RECIPIENT_BASE_SEPOLIA}" = ["royalty_distribution"]

[hooks.networks.base-sepolia.contracts]
royalty_distribution = "${ROYALTY_CONTRACT_BASE_SEPOLIA}"
```

**Environment Variables (.env):**
```bash
ROYALTY_TRIGGER_RECIPIENT_BASE_SEPOLIA=0xPaymentRecipientAddress
ROYALTY_CONTRACT_BASE_SEPOLIA=0xRoyaltyContractAddress
ROYALTY_RECIPIENT_ADDRESS=0x1111111111111111111111111111111111111111
```

### Example 5: Batch Context Tracking

Tracks settlement position within a batch for sequential processing.

**Solidity Interface:**
```solidity
interface IBatchTracker {
    function recordBatchSettlement(
        address from,
        uint256 amount,
        uint256 batchIndex,
        uint256 batchSize
    ) external;
}
```

**Configuration:**
```toml
# Hook definition (shared across networks)
[hooks.definitions.batch_tracker]
enabled = true
description = "Records settlement with batch context"
function_signature = "recordBatchSettlement(address,uint256,uint256,uint256)"
gas_limit = 100000

[[hooks.definitions.batch_tracker.parameters]]
type = "address"
source = { source_type = "payment", field = "from" }

[[hooks.definitions.batch_tracker.parameters]]
type = "uint256"
source = { source_type = "payment", field = "value" }

[[hooks.definitions.batch_tracker.parameters]]
type = "uint256"
source = { source_type = "runtime", field = "batchindex" }

[[hooks.definitions.batch_tracker.parameters]]
type = "uint256"
source = { source_type = "runtime", field = "batchsize" }

# Per-network configuration
[hooks.networks.base-sepolia]
enabled = true

[hooks.networks.base-sepolia.mappings]
"${BATCH_TRACKER_RECIPIENT_BASE_SEPOLIA}" = ["batch_tracker"]

[hooks.networks.base-sepolia.contracts]
batch_tracker = "${BATCH_TRACKER_CONTRACT_BASE_SEPOLIA}"
```

**Environment Variables (.env):**
```bash
BATCH_TRACKER_RECIPIENT_BASE_SEPOLIA=0xRecipientAddress
BATCH_TRACKER_CONTRACT_BASE_SEPOLIA=0xBatchTrackerAddress
```

## Admin API

Manage hooks at runtime without restarting the server:

### List All Hooks
```bash
curl -H "X-Admin-Key: your_admin_key" \
  http://localhost:8080/admin/hooks
```

### List Hook Mappings
```bash
curl -H "X-Admin-Key: your_admin_key" \
  http://localhost:8080/admin/hooks/mappings
```

### Reload Configuration
```bash
curl -X POST \
  -H "X-Admin-Key: your_admin_key" \
  http://localhost:8080/admin/hooks/reload
```

### Enable/Disable Specific Hook
```bash
# Enable
curl -X POST \
  -H "X-Admin-Key: your_admin_key" \
  http://localhost:8080/admin/hooks/hook_name/enable

# Disable
curl -X POST \
  -H "X-Admin-Key: your_admin_key" \
  http://localhost:8080/admin/hooks/hook_name/disable
```

### Check Hook Status
```bash
curl -H "X-Admin-Key: your_admin_key" \
  http://localhost:8080/admin/hooks/status
```

## Security Considerations

### Access Control
- All hook definitions must be manually added by admins via `hooks.toml`
- Only destination mappings can be modified via admin API (with authentication)
- Hook contract addresses are immutable once defined
- Admin API requires `X-Admin-Key` header authentication

### Gas Limits
- Set reasonable gas limits to prevent DoS attacks
- Monitor hook execution costs in production
- Consider batch size impact on total gas usage
- Use `allow_hook_failure = false` by default for safety

### Contract Validation
- Verify hook contract source code before deployment
- Test hooks thoroughly on testnet
- Ensure hook contracts handle failure gracefully
- Use established audit patterns for hook contracts

### Permission Model
- Hooks execute with facilitator's permissions
- Hook contracts can perform any action the facilitator can perform
- Treat hook contracts as highly privileged code
- Implement proper access controls in hook contracts

### Failure Handling
- `allow_hook_failure = false` (default): Any hook failure reverts entire batch
- `allow_hook_failure = true`: Individual hooks can fail without reverting transfers
- Failed hooks indicate potential issues -- investigate immediately
- Monitor hook execution success rates

## Testing Hooks

### Local Testing

1. Deploy hook contract to testnet
2. Add hook definition to `hooks.toml`
3. Add recipient mapping with per-network contracts
4. Enable hooks globally: `enabled = true`
5. Submit test payment to mapped recipient
6. Verify hook execution in transaction logs

### Unit Testing

See `src/hooks/manager.rs` tests for examples of testing hook encoding and execution.

### Integration Testing

1. Use testnet with test tokens
2. Monitor Multicall3 transaction for hook execution
3. Verify hook contract state changes
4. Test both success and failure scenarios
5. Validate batch behavior with multiple hooks

## Troubleshooting

### Hook Not Executing

1. Check `hooks.enabled = true` in hooks.toml
2. Verify hook definition has `enabled = true`
3. Confirm recipient address is in per-network mappings
4. Check admin logs for hook resolution
5. Verify gas limit is sufficient

### Hook Execution Failing

1. Check transaction logs for revert reason
2. Verify function signature matches contract ABI
3. Ensure parameter types match Solidity types
4. Test hook contract function directly
5. Increase gas limit if out-of-gas

### Parameter Encoding Issues

1. Verify field names match available sources (all lowercase)
2. Check Solidity types are correct
3. Ensure config values exist for `config` source
4. Validate static values parse correctly
5. Review logs for encoding errors

## Best Practices

1. **Start with testnet**: Always test hooks on testnet first
2. **Use descriptive names**: Make hook names and descriptions clear
3. **Document parameters**: Comment what each parameter represents
4. **Monitor execution**: Watch hook execution in production
5. **Keep gas low**: Minimize gas usage in hook contracts
6. **Handle failures**: Implement proper error handling
7. **Version control**: Track `hooks.toml` in git (use env vars for addresses)
8. **Audit contracts**: Get hook contracts audited before mainnet
9. **Test edge cases**: Try batch scenarios, failures, gas limits
10. **Keep it simple**: Avoid complex logic in hooks when possible

## Advanced Topics

### Multiple Hooks per Recipient

You can attach multiple hooks to a single recipient:

```toml
[hooks.networks.base-sepolia.mappings]
"${RECIPIENT}" = ["hook1", "hook2", "hook3"]
```

Hooks execute in array order. If `allow_hook_failure = false`, all hooks must succeed.

### Call3-Aware Batching

The batch processor counts total Multicall3 Call3 structs:

- Each settlement = 1 Call3 (transferWithAuthorization) + N Call3s (hooks)
- With `max_batch_size = 100`:
  - 30 settlements x 3 calls each (1 transfer + 2 hooks) = 90 Call3s
  - 100 settlements x 1 call each (transfer only) = 100 Call3s

### Conditional Hook Execution

To conditionally execute hooks, implement the logic in the hook contract itself:

```solidity
function onPaymentReceived(...) external {
    if (amount < MIN_AMOUNT) return;  // Skip small payments
    // ... hook logic
}
```

### Hook Composability

Hooks can call other contracts, enabling complex workflows:

```solidity
function onPaymentReceived(...) external {
    // Mint NFT
    nftContract.mint(from, tokenId);

    // Update registry
    registry.recordMint(from, tokenId, amount);

    // Emit event
    emit PaymentProcessed(from, tokenId, amount);
}
```

## Architecture

### Components

1. **Hook Configuration** (`src/hooks/config.rs`): TOML parsing and validation
2. **Hook Manager** (`src/hooks/manager.rs`): Thread-safe hot-reloadable manager
3. **Admin API** (`src/hooks/admin.rs`): REST endpoints for runtime management
4. **Runtime Context** (`src/hooks/context.rs`): Dynamic parameter resolution
5. **Batch Integration** (`src/batch/processor.rs`): Hook lookup and execution within Multicall3 batches

## Further Reading

- [Batch Settlement Guide](BATCH_SETTLEMENT.md) - How hooks integrate with batch settlement
- [Configuration Guide](CONFIGURATION.md) - Full configuration reference
- [API Reference](API.md) - Admin hook endpoints
- [Security Documentation](SECURITY.md) - Security best practices
