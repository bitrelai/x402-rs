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

/// Extract EIP-3009 settlement metadata from a raw SettleRequest JSON.
///
/// Handles both v1 and v2 payload formats:
/// - V2: `paymentPayload.authorization.*`, `paymentPayload.accepted.asset`, `paymentPayload.signature`
/// - V1: `paymentPayload.payload.authorization.*`, `paymentRequirements.asset`, `paymentPayload.payload.signature`
///
/// Returns None if the payload doesn't contain EIP-3009 authorization data.
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
                    // Verify first
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

                    // Extract metadata
                    let metadata = extract_eip3009_metadata(request).ok_or_else(|| {
                        BatchFacilitatorError::Batch(
                            "Failed to extract EIP-3009 metadata from request".into(),
                        )
                    })?;

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
