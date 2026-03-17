use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use x402_facilitator_local::FacilitatorLocal;
use x402_types::chain::ChainId;
use x402_types::facilitator::Facilitator;
use x402_types::proto;
use x402_types::scheme::SchemeRegistry;

use super::queue::BatchQueueManager;
use crate::hooks::types::SettlementMetadata;

#[cfg(feature = "chain-eip155")]
use x402_chain_eip155::chain::Eip155ChainProvider;

/// Enterprise facilitator that wraps upstream's `FacilitatorLocal` with optional
/// batch settlement support.
///
/// When batch settlement is enabled for a chain, `/settle` requests are queued
/// and processed as Multicall3 batches. Otherwise, requests pass through to
/// the upstream direct settlement path.
pub struct BatchFacilitator {
    /// Upstream facilitator for verify, supported, and direct settlement.
    pub inner: Arc<FacilitatorLocal<SchemeRegistry>>,
    /// Optional batch queue manager (None = batch disabled).
    pub batch_queue: Option<Arc<BatchQueueManager>>,
    /// Chain providers for accessing EVM providers (same Arcs as used by SchemeRegistry).
    #[cfg(feature = "chain-eip155")]
    pub evm_providers: HashMap<ChainId, Arc<Eip155ChainProvider>>,
}

#[derive(Debug)]
pub enum BatchFacilitatorError {
    Inner(x402_facilitator_local::FacilitatorLocalError),
    Batch(String),
}

impl fmt::Display for BatchFacilitatorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BatchFacilitatorError::Inner(e) => write!(f, "{}", e),
            BatchFacilitatorError::Batch(e) => write!(f, "Batch error: {}", e),
        }
    }
}

impl From<x402_facilitator_local::FacilitatorLocalError> for BatchFacilitatorError {
    fn from(e: x402_facilitator_local::FacilitatorLocalError) -> Self {
        BatchFacilitatorError::Inner(e)
    }
}

impl IntoResponse for BatchFacilitatorError {
    fn into_response(self) -> Response {
        match self {
            BatchFacilitatorError::Inner(e) => e.into_response(),
            BatchFacilitatorError::Batch(msg) => {
                let body = serde_json::json!({
                    "success": false,
                    "errorReason": "unexpected_error",
                    "errorReasonDetails": msg,
                });
                (StatusCode::INTERNAL_SERVER_ERROR, axum::Json(body)).into_response()
            }
        }
    }
}

/// Extract chain ID from a raw SettleRequest JSON.
/// Works with both v1 (network field) and v2 (accepted.network field).
fn extract_chain_id(request: &proto::SettleRequest) -> Option<ChainId> {
    request.scheme_handler_slug().map(|slug| slug.chain_id)
}

/// EIP-6492 magic suffix (32 bytes). Signatures ending with this are EIP-6492 wrapped.
const EIP6492_MAGIC_SUFFIX: [u8; 32] = [
    0x64, 0x92, 0x64, 0x92, 0x64, 0x92, 0x64, 0x92, 0x64, 0x92, 0x64, 0x92, 0x64, 0x92, 0x64,
    0x92, 0x64, 0x92, 0x64, 0x92, 0x64, 0x92, 0x64, 0x92, 0x64, 0x92, 0x64, 0x92, 0x64, 0x92,
    0x64, 0x92,
];

/// Check if a signature is EIP-6492 wrapped (ends with magic suffix).
fn is_eip6492_signature(sig: &[u8]) -> bool {
    sig.len() > 32 && sig[sig.len() - 32..] == EIP6492_MAGIC_SUFFIX
}

/// Extract EIP-3009 settlement metadata from a raw SettleRequest JSON.
///
/// Handles both v1 and v2 payload formats:
/// - V2: `paymentPayload.authorization.*`, `paymentPayload.accepted.asset`, `paymentPayload.signature`
/// - V1: `paymentPayload.payload.authorization.*`, `paymentRequirements.asset`, `paymentPayload.payload.signature`
///
/// Returns None if:
/// - The payload doesn't contain EIP-3009 authorization data (Permit2, upto, etc.)
/// - The signature is EIP-6492 wrapped (contains factory deployment data that the
///   batch path cannot handle — must fall back to upstream direct settlement)
/// - Any required field is missing or malformed
fn extract_eip3009_metadata(request: &proto::SettleRequest) -> Option<SettlementMetadata> {
    use alloy::primitives::{Address, Bytes, FixedBytes};

    let value: serde_json::Value = serde_json::from_str(request.as_str()).ok()?;
    let payment_payload = value.get("paymentPayload")?;

    // Try V2 first: authorization at paymentPayload.authorization
    // Then V1: authorization at paymentPayload.payload.authorization
    let (authorization, signature_obj, asset_str) =
        if let Some(auth) = payment_payload.get("authorization") {
            // V2 format
            let sig = payment_payload;
            let asset = payment_payload.get("accepted")?.get("asset")?.as_str()?;
            (auth, sig, asset)
        } else if let Some(inner_payload) = payment_payload.get("payload") {
            // V1 format: authorization and signature inside payload
            let auth = inner_payload.get("authorization")?;
            let asset = value.get("paymentRequirements")?.get("asset")?.as_str()?;
            (auth, inner_payload, asset)
        } else {
            return None;
        };

    let from: Address = authorization.get("from")?.as_str()?.parse().ok()?;
    let to: Address = authorization.get("to")?.as_str()?.parse().ok()?;

    let value_u256 = parse_u256_field(authorization.get("value")?)?;
    let valid_after = parse_u256_field(authorization.get("validAfter")?)?;
    let valid_before = parse_u256_field(authorization.get("validBefore")?)?;

    let nonce_str = authorization.get("nonce")?.as_str()?;
    let nonce: FixedBytes<32> = nonce_str.parse().ok()?;

    let signature_str = signature_obj.get("signature")?.as_str()?;
    let signature: Bytes = signature_str.parse().ok()?;

    // Reject EIP-6492 wrapped signatures — they contain factory deployment data
    // that the batch path cannot handle. These must go through upstream direct
    // settlement which has full EIP-6492 support.
    if is_eip6492_signature(&signature) {
        tracing::debug!("EIP-6492 signature detected — not batchable");
        return None;
    }

    let contract_address: Address = asset_str.parse().ok()?;

    Some(SettlementMetadata {
        from,
        to,
        value: value_u256,
        valid_after,
        valid_before,
        nonce,
        signature,
        contract_address,
    })
}

/// Parse a U256 from a JSON value that may be a string (decimal or hex) or a number.
fn parse_u256_field(val: &serde_json::Value) -> Option<alloy::primitives::U256> {
    use alloy::primitives::U256;

    if let Some(s) = val.as_str() {
        U256::from_str_radix(s, 10)
            .or_else(|_| U256::from_str_radix(s.trim_start_matches("0x"), 16))
            .ok()
    } else if let Some(n) = val.as_u64() {
        Some(U256::from(n))
    } else {
        None
    }
}

/// Extract the network name from a chain ID (e.g., "eip155:8453" -> "base").
fn chain_id_to_network_name(chain_id: &ChainId) -> String {
    x402_types::networks::network_name_by_chain_id(chain_id)
        .map(|s| s.to_string())
        .unwrap_or_else(|| chain_id.to_string())
}

impl Facilitator for BatchFacilitator {
    type Error = BatchFacilitatorError;

    async fn verify(
        &self,
        request: &proto::VerifyRequest,
    ) -> Result<proto::VerifyResponse, Self::Error> {
        self.inner.verify(request).await.map_err(Into::into)
    }

    async fn settle(
        &self,
        request: &proto::SettleRequest,
    ) -> Result<proto::SettleResponse, Self::Error> {
        // Check if we should batch this request
        #[cfg(feature = "chain-eip155")]
        if let Some(ref batch_queue) = self.batch_queue {
            if let Some(chain_id) = extract_chain_id(request) {
                let network_name = chain_id_to_network_name(&chain_id);

                if chain_id.namespace() == "eip155" && batch_queue.should_batch(&network_name) {
                    // Try to extract EIP-3009 metadata. If extraction fails (Permit2,
                    // upto, EIP-6492 wrapped, or unrecognized scheme), fall back to
                    // upstream direct settlement instead of erroring.
                    let metadata = match extract_eip3009_metadata(request) {
                        Some(m) => m,
                        None => {
                            tracing::debug!(
                                network = %network_name,
                                "Request is not EIP-3009 exact — falling back to direct settlement"
                            );
                            return self.inner.settle(request).await.map_err(Into::into);
                        }
                    };

                    // Verify before enqueueing
                    let verify_response = self.inner.verify(request).await?;
                    let is_valid = verify_response
                        .0
                        .get("isValid")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);

                    if !is_valid {
                        return Err(BatchFacilitatorError::Batch(
                            "Verification failed before batch enqueue".into(),
                        ));
                    }

                    // Get the EVM provider for this chain
                    let provider =
                        self.evm_providers.get(&chain_id).ok_or_else(|| {
                            BatchFacilitatorError::Batch(format!(
                                "No EVM provider for chain {}",
                                chain_id
                            ))
                        })?;

                    // Enqueue and await result
                    let rx = batch_queue
                        .enqueue(network_name.clone(), metadata.clone(), Arc::clone(provider))
                        .await;

                    match rx.await {
                        Ok(Ok(result)) => {
                            // Convert BatchSettleResult to proto::SettleResponse
                            let response = serde_json::json!({
                                "success": result.success,
                                "transaction": result.tx_hash,
                                "network": network_name,
                                "payer": format!("{:?}", metadata.from),
                            });
                            return Ok(proto::SettleResponse(response));
                        }
                        Ok(Err(e)) => {
                            return Err(BatchFacilitatorError::Batch(e.to_string()));
                        }
                        Err(_) => {
                            return Err(BatchFacilitatorError::Batch(
                                "Batch response channel dropped".into(),
                            ));
                        }
                    }
                }
            }
        }

        // Fallback: direct settlement via upstream
        self.inner.settle(request).await.map_err(Into::into)
    }

    async fn supported(&self) -> Result<proto::SupportedResponse, Self::Error> {
        self.inner.supported().await.map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::U256;

    /// Helper: build a proto::SettleRequest from a JSON string.
    fn make_request(json: &str) -> proto::SettleRequest {
        let raw = serde_json::value::RawValue::from_string(json.to_string()).unwrap();
        proto::SettleRequest::from(raw)
    }

    // -----------------------------------------------------------------------
    // parse_u256_field
    // -----------------------------------------------------------------------

    #[test]
    fn parse_u256_field_decimal_string() {
        let val = serde_json::json!("1000000");
        let result = parse_u256_field(&val).unwrap();
        assert_eq!(result, U256::from(1_000_000u64));
    }

    #[test]
    fn parse_u256_field_hex_string() {
        let val = serde_json::json!("0xff");
        let result = parse_u256_field(&val).unwrap();
        assert_eq!(result, U256::from(255u64));
    }

    #[test]
    fn parse_u256_field_number() {
        let val = serde_json::json!(42u64);
        let result = parse_u256_field(&val).unwrap();
        assert_eq!(result, U256::from(42u64));
    }

    #[test]
    fn parse_u256_field_zero_string() {
        let val = serde_json::json!("0");
        let result = parse_u256_field(&val).unwrap();
        assert_eq!(result, U256::ZERO);
    }

    #[test]
    fn parse_u256_field_large_decimal_string() {
        // U256 MAX = 115792089237316195423570985008687907853269984665640564039457584007913129639935
        let max_str = "115792089237316195423570985008687907853269984665640564039457584007913129639935";
        let val = serde_json::json!(max_str);
        let result = parse_u256_field(&val).unwrap();
        assert_eq!(result, U256::MAX);
    }

    #[test]
    fn parse_u256_field_boolean_returns_none() {
        let val = serde_json::json!(true);
        assert!(parse_u256_field(&val).is_none());
    }

    #[test]
    fn parse_u256_field_null_returns_none() {
        let val = serde_json::json!(null);
        assert!(parse_u256_field(&val).is_none());
    }

    #[test]
    fn parse_u256_field_invalid_string_returns_none() {
        let val = serde_json::json!("not_a_number");
        assert!(parse_u256_field(&val).is_none());
    }

    // -----------------------------------------------------------------------
    // extract_chain_id
    // -----------------------------------------------------------------------

    #[test]
    fn extract_chain_id_v2_eip155() {
        let json = r#"{
            "x402Version": 2,
            "paymentPayload": {
                "accepted": {
                    "network": "eip155:84532",
                    "scheme": "exact",
                    "asset": "0x036CbD53842c5426634e7929541eC2318f3dCF7e",
                    "amount": "1000000"
                },
                "authorization": {
                    "from": "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    "to": "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                    "value": "1000000",
                    "validAfter": "0",
                    "validBefore": "115792089237316195423570985008687907853269984665640564039457584007913129639935",
                    "nonce": "0x0000000000000000000000000000000000000000000000000000000000000001"
                },
                "signature": "0xdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef00"
            }
        }"#;

        let request = make_request(json);
        let chain_id = extract_chain_id(&request).unwrap();
        assert_eq!(chain_id.namespace, "eip155");
        assert_eq!(chain_id.reference, "84532");
    }

    #[test]
    fn extract_chain_id_v1_network_name() {
        let json = r#"{
            "x402Version": 1,
            "paymentPayload": {
                "scheme": "exact",
                "network": "base-sepolia",
                "payload": {
                    "signature": "0xdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef00",
                    "authorization": {
                        "from": "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                        "to": "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                        "value": "1000000",
                        "validAfter": "0",
                        "validBefore": "115792089237316195423570985008687907853269984665640564039457584007913129639935",
                        "nonce": "0x0000000000000000000000000000000000000000000000000000000000000001"
                    }
                }
            },
            "paymentRequirements": {
                "scheme": "exact",
                "network": "base-sepolia",
                "asset": "0x036CbD53842c5426634e7929541eC2318f3dCF7e",
                "amount": "1000000",
                "payTo": "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
            }
        }"#;

        let request = make_request(json);
        let chain_id = extract_chain_id(&request).unwrap();
        assert_eq!(chain_id.namespace, "eip155");
        assert_eq!(chain_id.reference, "84532");
    }

    #[test]
    fn extract_chain_id_invalid_json_returns_none() {
        let json = r#"{"garbage": true}"#;
        let request = make_request(json);
        assert!(extract_chain_id(&request).is_none());
    }

    // -----------------------------------------------------------------------
    // chain_id_to_network_name
    // -----------------------------------------------------------------------

    #[test]
    fn chain_id_to_network_name_known_chain() {
        let chain_id = ChainId::new("eip155", "84532");
        assert_eq!(chain_id_to_network_name(&chain_id), "base-sepolia");
    }

    #[test]
    fn chain_id_to_network_name_base_mainnet() {
        let chain_id = ChainId::new("eip155", "8453");
        assert_eq!(chain_id_to_network_name(&chain_id), "base");
    }

    #[test]
    fn chain_id_to_network_name_unknown_chain_returns_chain_id_string() {
        let chain_id = ChainId::new("eip155", "999999");
        // Unknown chains should fall back to chain_id.to_string()
        assert_eq!(chain_id_to_network_name(&chain_id), "eip155:999999");
    }

    // -----------------------------------------------------------------------
    // extract_eip3009_metadata - V2 format
    // -----------------------------------------------------------------------

    #[test]
    fn extract_eip3009_metadata_v2_success() {
        use alloy::primitives::Address;

        let json = r#"{
            "x402Version": 2,
            "paymentPayload": {
                "accepted": {
                    "network": "eip155:84532",
                    "scheme": "exact",
                    "asset": "0x036CbD53842c5426634e7929541eC2318f3dCF7e",
                    "amount": "1000000"
                },
                "authorization": {
                    "from": "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    "to": "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                    "value": "1000000",
                    "validAfter": "0",
                    "validBefore": "115792089237316195423570985008687907853269984665640564039457584007913129639935",
                    "nonce": "0x0000000000000000000000000000000000000000000000000000000000000001"
                },
                "signature": "0xdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef00"
            }
        }"#;

        let request = make_request(json);
        let meta = extract_eip3009_metadata(&request).unwrap();

        let expected_from: Address = "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".parse().unwrap();
        let expected_to: Address = "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".parse().unwrap();
        let expected_asset: Address = "0x036CbD53842c5426634e7929541eC2318f3dCF7e".parse().unwrap();

        assert_eq!(meta.from, expected_from);
        assert_eq!(meta.to, expected_to);
        assert_eq!(meta.value, U256::from(1_000_000u64));
        assert_eq!(meta.valid_after, U256::ZERO);
        assert_eq!(meta.valid_before, U256::MAX);
        assert_eq!(meta.contract_address, expected_asset);
        // Nonce should be 1
        assert_eq!(meta.nonce[31], 1u8);
        // Signature should be 65 bytes
        assert_eq!(meta.signature.len(), 65);
    }

    // -----------------------------------------------------------------------
    // extract_eip3009_metadata - V1 format
    // -----------------------------------------------------------------------

    #[test]
    fn extract_eip3009_metadata_v1_success() {
        use alloy::primitives::Address;

        let json = r#"{
            "x402Version": 1,
            "paymentPayload": {
                "scheme": "exact",
                "network": "base-sepolia",
                "payload": {
                    "signature": "0xdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef00",
                    "authorization": {
                        "from": "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                        "to": "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                        "value": "1000000",
                        "validAfter": "0",
                        "validBefore": "115792089237316195423570985008687907853269984665640564039457584007913129639935",
                        "nonce": "0x0000000000000000000000000000000000000000000000000000000000000001"
                    }
                }
            },
            "paymentRequirements": {
                "scheme": "exact",
                "network": "base-sepolia",
                "asset": "0x036CbD53842c5426634e7929541eC2318f3dCF7e",
                "amount": "1000000",
                "payTo": "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
            }
        }"#;

        let request = make_request(json);
        let meta = extract_eip3009_metadata(&request).unwrap();

        let expected_from: Address = "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".parse().unwrap();
        let expected_to: Address = "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".parse().unwrap();
        let expected_asset: Address = "0x036CbD53842c5426634e7929541eC2318f3dCF7e".parse().unwrap();

        assert_eq!(meta.from, expected_from);
        assert_eq!(meta.to, expected_to);
        assert_eq!(meta.value, U256::from(1_000_000u64));
        assert_eq!(meta.valid_after, U256::ZERO);
        assert_eq!(meta.valid_before, U256::MAX);
        assert_eq!(meta.contract_address, expected_asset);
        assert_eq!(meta.nonce[31], 1u8);
        assert_eq!(meta.signature.len(), 65);
    }

    #[test]
    fn extract_eip3009_metadata_missing_authorization_returns_none() {
        let json = r#"{
            "x402Version": 2,
            "paymentPayload": {
                "accepted": {
                    "network": "eip155:84532",
                    "scheme": "exact",
                    "asset": "0x036CbD53842c5426634e7929541eC2318f3dCF7e",
                    "amount": "1000000"
                },
                "signature": "0xdeadbeef"
            }
        }"#;

        let request = make_request(json);
        // No authorization block -> None
        assert!(extract_eip3009_metadata(&request).is_none());
    }

    #[test]
    fn extract_eip3009_metadata_invalid_address_returns_none() {
        let json = r#"{
            "x402Version": 2,
            "paymentPayload": {
                "accepted": {
                    "network": "eip155:84532",
                    "scheme": "exact",
                    "asset": "0x036CbD53842c5426634e7929541eC2318f3dCF7e",
                    "amount": "1000000"
                },
                "authorization": {
                    "from": "not_an_address",
                    "to": "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                    "value": "1000000",
                    "validAfter": "0",
                    "validBefore": "1",
                    "nonce": "0x0000000000000000000000000000000000000000000000000000000000000001"
                },
                "signature": "0xdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef00"
            }
        }"#;

        let request = make_request(json);
        assert!(extract_eip3009_metadata(&request).is_none());
    }

    #[test]
    fn extract_eip3009_metadata_garbage_json_returns_none() {
        let json = r#"{"random": "data"}"#;
        let request = make_request(json);
        assert!(extract_eip3009_metadata(&request).is_none());
    }

    #[test]
    fn extract_eip3009_metadata_v1_missing_payment_requirements_returns_none() {
        // V1 format but missing paymentRequirements -> cannot find asset
        let json = r#"{
            "x402Version": 1,
            "paymentPayload": {
                "scheme": "exact",
                "network": "base-sepolia",
                "payload": {
                    "signature": "0xdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef00",
                    "authorization": {
                        "from": "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                        "to": "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                        "value": "1000000",
                        "validAfter": "0",
                        "validBefore": "1",
                        "nonce": "0x0000000000000000000000000000000000000000000000000000000000000001"
                    }
                }
            }
        }"#;

        let request = make_request(json);
        assert!(extract_eip3009_metadata(&request).is_none());
    }

    #[test]
    fn extract_eip3009_metadata_v2_missing_asset_returns_none() {
        // V2 format but accepted block has no "asset"
        let json = r#"{
            "x402Version": 2,
            "paymentPayload": {
                "accepted": {
                    "network": "eip155:84532",
                    "scheme": "exact",
                    "amount": "1000000"
                },
                "authorization": {
                    "from": "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    "to": "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                    "value": "1000000",
                    "validAfter": "0",
                    "validBefore": "1",
                    "nonce": "0x0000000000000000000000000000000000000000000000000000000000000001"
                },
                "signature": "0xdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef00"
            }
        }"#;

        let request = make_request(json);
        assert!(extract_eip3009_metadata(&request).is_none());
    }

    // -----------------------------------------------------------------------
    // EIP-6492 signature detection
    // -----------------------------------------------------------------------

    #[test]
    fn is_eip6492_signature_detects_magic_suffix() {
        // Build a signature that ends with the EIP-6492 magic suffix
        let mut sig = vec![0xaa; 65]; // normal sig bytes
        sig.extend_from_slice(&EIP6492_MAGIC_SUFFIX);
        assert!(is_eip6492_signature(&sig));
    }

    #[test]
    fn is_eip6492_signature_rejects_normal_signature() {
        let sig = vec![0xaa; 65]; // normal 65-byte EOA signature
        assert!(!is_eip6492_signature(&sig));
    }

    #[test]
    fn is_eip6492_signature_rejects_short_signature() {
        let sig = EIP6492_MAGIC_SUFFIX.to_vec(); // exactly 32 bytes = too short
        assert!(!is_eip6492_signature(&sig));
    }

    #[test]
    fn extract_eip3009_metadata_rejects_eip6492_wrapped_v2() {
        // Build a hex signature string that ends with the EIP-6492 magic suffix
        let mut sig_bytes = vec![0xaa; 65];
        sig_bytes.extend_from_slice(&EIP6492_MAGIC_SUFFIX);
        let sig_hex = format!("0x{}", alloy::hex::encode(&sig_bytes));

        let json = format!(r#"{{
            "x402Version": 2,
            "paymentPayload": {{
                "accepted": {{
                    "network": "eip155:84532",
                    "scheme": "exact",
                    "asset": "0x036CbD53842c5426634e7929541eC2318f3dCF7e",
                    "amount": "1000000"
                }},
                "authorization": {{
                    "from": "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    "to": "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                    "value": "1000000",
                    "validAfter": "0",
                    "validBefore": "115792089237316195423570985008687907853269984665640564039457584007913129639935",
                    "nonce": "0x0000000000000000000000000000000000000000000000000000000000000001"
                }},
                "signature": "{sig_hex}"
            }}
        }}"#);

        let request = make_request(&json);
        // Should return None because of EIP-6492 magic suffix
        assert!(
            extract_eip3009_metadata(&request).is_none(),
            "EIP-6492 wrapped signature should cause metadata extraction to return None"
        );
    }
}
