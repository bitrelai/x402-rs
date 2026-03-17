# API Reference

Complete reference for all HTTP endpoints exposed by the enterprise facilitator.

## Base URL

```
http://localhost:8080
```

In production, use your domain with HTTPS:
```
https://facilitator.yourdomain.com
```

## Authentication

### API Key Authentication

Payment endpoints (`/verify`, `/settle`) require API key authentication when the `API_KEYS` environment variable is set.

Include the API key in the `Authorization` header:

```
Authorization: Bearer <your-api-key>
```

### Admin Authentication

Admin endpoints (`/admin/*`) require admin key authentication via the `X-Admin-Key` header:

```
X-Admin-Key: <your-admin-key>
```

## Public Endpoints

### GET /

**Description**: Root endpoint with API information and links to all available endpoints.

**Response**: HTML page listing protocol and admin endpoints with version information.

**Example**:
```bash
curl http://localhost:8080/
```

### GET /health

**Description**: Health check endpoint returning supported networks and payment schemes.

**Response**:
```json
{
  "kinds": [
    {
      "version": "1.0",
      "scheme": "ERC-3009-TransferWithAuthorization",
      "network": "base-sepolia"
    },
    {
      "version": "1.0",
      "scheme": "ERC-3009-TransferWithAuthorization",
      "network": "bsc"
    }
  ]
}
```

**Example**:
```bash
curl http://localhost:8080/health
```

### GET /supported

**Description**: Lists all supported payment schemes and blockchain networks.

**Response**: Same format as `/health`

**Example**:
```bash
curl http://localhost:8080/supported
```

## Payment Endpoints

### POST /verify

**Description**: Verify a payment payload's cryptographic signature and validate against payment requirements.

**Authentication**: API key required (if configured)

**Request Body**:
```json
{
  "paymentPayload": {
    "version": "1.0",
    "scheme": "ERC-3009-TransferWithAuthorization",
    "network": "base-sepolia",
    "from": "0x1234567890123456789012345678901234567890",
    "to": "0x0987654321098765432109876543210987654321",
    "value": "1000000",
    "validAfter": "0",
    "validBefore": "999999999999",
    "nonce": "0x0000000000000000000000000000000000000000000000000000000000000001",
    "signature": "0x..."
  },
  "paymentRequirements": {
    "version": "1.0",
    "scheme": "ERC-3009-TransferWithAuthorization",
    "network": "base-sepolia",
    "to": "0x0987654321098765432109876543210987654321",
    "value": "1000000"
  }
}
```

**Success Response** (200 OK):
```json
{
  "valid": true,
  "payer": "0x1234567890123456789012345678901234567890"
}
```

**Failure Response** (200 OK):
```json
{
  "valid": false,
  "payer": "0x1234567890123456789012345678901234567890",
  "reason": {
    "type": "InvalidSignature",
    "message": "Signature verification failed"
  }
}
```

**Error Response** (400 Bad Request):
```json
{
  "error": "Invalid request"
}
```

**Example**:
```bash
curl -X POST http://localhost:8080/verify \
  -H "Authorization: Bearer your-api-key" \
  -H "Content-Type: application/json" \
  -d '{
    "paymentPayload": {...},
    "paymentRequirements": {...}
  }'
```

### POST /settle

**Description**: Settle a verified payment by submitting it to the blockchain. When batch settlement is enabled and the request uses the EIP-3009 exact scheme, the request is queued and settled as part of a Multicall3 batch. Otherwise, the request is settled directly via the upstream facilitator.

**Authentication**: API key required (if configured)

**Request Body**: Same format as `/verify`

**Success Response** (200 OK) -- direct settlement:
```json
{
  "success": true,
  "payer": "0x1234567890123456789012345678901234567890",
  "transaction": "0xabcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890",
  "network": "base-sepolia"
}
```

**Success Response** (200 OK) -- batch settlement:
```json
{
  "success": true,
  "payer": "0x1234567890123456789012345678901234567890",
  "transaction": "0xabcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890",
  "network": "base-sepolia"
}
```

**Failure Response** (200 OK):
```json
{
  "success": false,
  "payer": "0x1234567890123456789012345678901234567890",
  "error_reason": {
    "type": "InsufficientFunds",
    "message": "Payer has insufficient balance"
  },
  "transaction": null,
  "network": "base-sepolia"
}
```

**Example**:
```bash
curl -X POST http://localhost:8080/settle \
  -H "Authorization: Bearer your-api-key" \
  -H "Content-Type: application/json" \
  -d '{
    "paymentPayload": {...},
    "paymentRequirements": {...}
  }'
```

## Admin Endpoints

All admin endpoints require the `X-Admin-Key` header.

### GET /admin/stats

**Description**: Returns statistics about abuse detection and batch settlement queues.

**Response**:
```json
{
  "abuse": {
    "total_ips_tracked": 42,
    "suspicious_ips": 3
  },
  "batch": {
    "enabled": true,
    "active_queues": 5
  }
}
```

**Example**:
```bash
curl http://localhost:8080/admin/stats \
  -H "X-Admin-Key: your-admin-key"
```

### GET /admin/hooks

**Description**: Lists all hook definitions.

**Note**: Only available when hook manager is initialized (hooks.toml exists and is valid).

**Example**:
```bash
curl http://localhost:8080/admin/hooks \
  -H "X-Admin-Key: your-admin-key"
```

### GET /admin/hooks/mappings

**Description**: Lists destination-to-hook mappings for all networks.

**Example**:
```bash
curl http://localhost:8080/admin/hooks/mappings \
  -H "X-Admin-Key: your-admin-key"
```

### GET /admin/hooks/status

**Description**: Returns hook system status including enabled state and hook/mapping counts.

**Response**:
```json
{
  "enabled": true,
  "hooks_count": 2,
  "mappings_count": 3
}
```

**Example**:
```bash
curl http://localhost:8080/admin/hooks/status \
  -H "X-Admin-Key: your-admin-key"
```

### POST /admin/hooks/reload

**Description**: Hot-reload hook configuration from `hooks.toml` without restarting the server.

**Response**:
```json
{
  "success": true,
  "message": "Reloaded 2 hook definitions"
}
```

**Example**:
```bash
curl -X POST http://localhost:8080/admin/hooks/reload \
  -H "X-Admin-Key: your-admin-key"
```

### POST /admin/hooks/{name}/enable

**Description**: Enable a specific hook by name.

**Example**:
```bash
curl -X POST http://localhost:8080/admin/hooks/token_mint_callback/enable \
  -H "X-Admin-Key: your-admin-key"
```

### POST /admin/hooks/{name}/disable

**Description**: Disable a specific hook by name.

**Example**:
```bash
curl -X POST http://localhost:8080/admin/hooks/token_mint_callback/disable \
  -H "X-Admin-Key: your-admin-key"
```

### POST /admin/tokens/reload

**Description**: Hot-reload token configuration from `tokens.toml` without restarting the server.

**Note**: Only available when token manager is initialized (tokens.toml exists and is valid).

**Response**:
```json
{
  "success": true,
  "message": "Token configuration reloaded"
}
```

**Example**:
```bash
curl -X POST http://localhost:8080/admin/tokens/reload \
  -H "X-Admin-Key: your-admin-key"
```

## Error Responses

### 400 Bad Request

**Cause**: Malformed request body or invalid parameters

**Response**:
```json
{
  "error": "Invalid request"
}
```

### 401 Unauthorized

**Cause**: Missing or invalid API key / admin key

**Response**:
```json
{
  "error": "Unauthorized"
}
```

### 403 Forbidden

**Cause**: IP address is blocked or banned

**Response**:
```json
{
  "error": "Forbidden"
}
```

### 429 Too Many Requests

**Cause**: Rate limit exceeded

**Response**:
```json
{
  "error": "Rate limit exceeded"
}
```

**Headers**:
```
Retry-After: 60
```

### 413 Payload Too Large

**Cause**: Request body exceeds `max_body_size_bytes` configuration

**Response**:
```json
{
  "error": "Payload too large"
}
```

## Rate Limiting

When rate limiting is enabled:
- Each IP address is tracked independently
- Limits are enforced per endpoint (or globally if no endpoint override)
- Exceeding the limit returns `429 Too Many Requests`
- After `ban_threshold` violations, IP is temporarily banned (403 Forbidden)
- Bans expire after `ban_duration_seconds`

See [Configuration Guide](CONFIGURATION.md#rate-limiting-configuration) for rate limit configuration.

## CORS

Cross-origin requests are controlled via the `cors` section in `config.toml`. When `allowed_origins` is empty, all origins are permitted (`*`). For production, specify exact origins.

See [Configuration Guide](CONFIGURATION.md#cors-configuration) for CORS configuration.

## Payload Limits

Maximum request body size is controlled by `request.max_body_size_bytes` in `config.toml` (default: 1 MB).

## SDK Compatibility

This facilitator is compatible with official x402 client SDKs:
- TypeScript SDK
- Go SDK
- Rust SDK (x402-rs)

All payloads follow the x402 protocol specification defined in the `x402-rs` crate.

## Further Reading

- [x402 Protocol Documentation](https://x402.org) - Protocol specification and flow
- [Configuration Guide](CONFIGURATION.md) - Security and rate limiting configuration
- [Security Documentation](SECURITY.md) - Security best practices
- [Deployment Guide](DEPLOYMENT.md) - Production deployment checklist
