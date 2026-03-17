use alloy::primitives::{Address, Bytes, U256};
use alloy::sol_types::SolCall;
use std::sync::Arc;

use super::multicall3::{self, IMulticall3, MULTICALL3_ADDRESS};
use crate::hooks::types::SettlementMetadata;
use crate::hooks::{HookCall, HookManager, RuntimeContext};

#[cfg(feature = "chain-eip155")]
use x402_chain_eip155::chain::{Eip155ChainProvider, Eip155MetaTransactionProvider, MetaTransaction};

/// A single entry in the batch queue waiting for settlement.
pub struct BatchEntry {
    /// Settlement metadata extracted from the request payload.
    pub metadata: SettlementMetadata,
    /// Network name for hook resolution.
    pub network: String,
    /// Response channel to send the result back to the caller.
    pub response_tx: tokio::sync::oneshot::Sender<Result<BatchSettleResult, BatchError>>,
}

/// Result of a single settlement within a batch.
#[derive(Debug, Clone)]
pub struct BatchSettleResult {
    /// Whether this individual settlement succeeded.
    pub success: bool,
    /// Transaction hash of the batch transaction.
    pub tx_hash: String,
}

/// Errors during batch processing.
#[derive(Debug, thiserror::Error)]
pub enum BatchError {
    #[error("Failed to encode calldata: {0}")]
    Encoding(String),
    #[error("Transaction failed: {0}")]
    Transaction(String),
    #[error("Hook resolution failed: {0}")]
    Hook(String),
    #[error("Provider not available for chain")]
    ProviderNotAvailable,
}

/// Build a `transferWithAuthorization` Call3 entry from settlement metadata.
fn build_transfer_call3(
    metadata: &SettlementMetadata,
    allow_failure: bool,
) -> IMulticall3::Call3 {
    let call = multicall3::transferWithAuthorizationCall {
        from: metadata.from,
        to: metadata.to,
        value: metadata.value,
        validAfter: metadata.valid_after,
        validBefore: metadata.valid_before,
        nonce: metadata.nonce,
        signature: metadata.signature.clone(),
    };
    let calldata = alloy::sol_types::SolCall::abi_encode(&call);

    IMulticall3::Call3 {
        target: metadata.contract_address,
        allowFailure: allow_failure,
        callData: Bytes::from(calldata),
    }
}

/// Build a hook Call3 entry.
fn build_hook_call3(hook: &HookCall) -> IMulticall3::Call3 {
    IMulticall3::Call3 {
        target: hook.target,
        allowFailure: hook.allow_failure,
        callData: hook.calldata.clone(),
    }
}

/// Process a batch of settlements via Multicall3.
///
/// Builds Call3 entries for each settlement (transfer + hooks), encodes an
/// `aggregate3()` call, and submits it through the upstream provider's
/// `send_transaction(MetaTransaction)` API.
#[cfg(feature = "chain-eip155")]
pub async fn process_batch(
    provider: &Arc<Eip155ChainProvider>,
    entries: Vec<BatchEntry>,
    hook_manager: Option<&Arc<HookManager>>,
    allow_partial_failure: bool,
) -> Result<String, BatchError> {
    if entries.is_empty() {
        return Err(BatchError::Encoding("empty batch".into()));
    }

    let mut all_calls: Vec<IMulticall3::Call3> = Vec::new();
    let mut entry_call_ranges: Vec<(usize, usize)> = Vec::new(); // (start, count) per entry

    // Get the first signer address for runtime context
    let signer_addresses = provider.signer_addresses();
    let sender = signer_addresses
        .first()
        .and_then(|s| s.parse::<Address>().ok())
        .unwrap_or(Address::ZERO);

    for entry in &entries {
        let start = all_calls.len();

        // Add transfer Call3
        all_calls.push(build_transfer_call3(&entry.metadata, allow_partial_failure));

        // Add hook Call3s if hook manager is available
        if let Some(hm) = hook_manager {
            let runtime = RuntimeContext::new(
                U256::ZERO, // Will be filled by block at execution time
                U256::ZERO,
                sender,
            );

            match hm
                .get_hooks_for_destination_with_context(
                    entry.metadata.to,
                    entry.metadata.contract_address,
                    &entry.network,
                    &entry.metadata,
                    &runtime,
                )
                .await
            {
                Ok(hooks) => {
                    for hook in &hooks {
                        all_calls.push(build_hook_call3(hook));
                    }
                }
                Err(e) => {
                    tracing::error!(error = %e, "Failed to resolve hooks for batch entry");
                    return Err(BatchError::Hook(e.to_string()));
                }
            }
        }

        let count = all_calls.len() - start;
        entry_call_ranges.push((start, count));
    }

    // Encode Multicall3 aggregate3() calldata
    let aggregate_call = IMulticall3::aggregate3Call {
        calls: all_calls,
    };
    let calldata = Bytes::from(aggregate_call.abi_encode());

    // Submit via upstream provider's MetaTransaction API
    let meta_tx = MetaTransaction::new(MULTICALL3_ADDRESS, calldata);

    let receipt = provider
        .send_transaction(meta_tx)
        .await
        .map_err(|e| BatchError::Transaction(e.to_string()))?;

    let tx_hash = format!("{:?}", receipt.transaction_hash);
    let batch_success = receipt.status();

    tracing::info!(
        tx_hash = %tx_hash,
        success = batch_success,
        entries = entries.len(),
        "Batch settlement transaction submitted"
    );

    // Route results back to callers
    for (i, entry) in entries.into_iter().enumerate() {
        let result = BatchSettleResult {
            success: batch_success,
            tx_hash: tx_hash.clone(),
        };
        let _ = entry.response_tx.send(Ok(result));
        let _ = i; // suppress unused warning
    }

    Ok(tx_hash)
}

/// Signer addresses accessor (needed since the trait method returns Vec<String>).
#[cfg(feature = "chain-eip155")]
trait SignerAddresses {
    fn signer_addresses(&self) -> Vec<String>;
}

#[cfg(feature = "chain-eip155")]
impl SignerAddresses for Eip155ChainProvider {
    fn signer_addresses(&self) -> Vec<String> {
        x402_types::chain::ChainProviderOps::signer_addresses(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{address, bytes, fixed_bytes, U256};
    use alloy::sol_types::SolCall;

    /// Create a sample SettlementMetadata for testing.
    fn sample_metadata() -> SettlementMetadata {
        SettlementMetadata {
            from: address!("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            to: address!("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
            value: U256::from(1_000_000u64),
            valid_after: U256::ZERO,
            valid_before: U256::MAX,
            nonce: fixed_bytes!("0000000000000000000000000000000000000000000000000000000000000001"),
            signature: bytes!("deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef00"),
            contract_address: address!("036CbD53842c5426634e7929541eC2318f3dCF7e"),
        }
    }

    // -----------------------------------------------------------------------
    // build_transfer_call3
    // -----------------------------------------------------------------------

    #[test]
    fn build_transfer_call3_target_is_contract_address() {
        let meta = sample_metadata();
        let call3 = build_transfer_call3(&meta, false);
        assert_eq!(call3.target, meta.contract_address);
    }

    #[test]
    fn build_transfer_call3_allow_failure_false() {
        let meta = sample_metadata();
        let call3 = build_transfer_call3(&meta, false);
        assert!(!call3.allowFailure);
    }

    #[test]
    fn build_transfer_call3_allow_failure_true() {
        let meta = sample_metadata();
        let call3 = build_transfer_call3(&meta, true);
        assert!(call3.allowFailure);
    }

    #[test]
    fn build_transfer_call3_calldata_starts_with_selector() {
        let meta = sample_metadata();
        let call3 = build_transfer_call3(&meta, false);

        // transferWithAuthorization selector: first 4 bytes of keccak256 of the function signature
        // The calldata must be at least 4 bytes (selector) + encoded parameters
        assert!(call3.callData.len() > 4);

        // The calldata should be decodable as transferWithAuthorizationCall
        let decoded = multicall3::transferWithAuthorizationCall::abi_decode(&call3.callData);
        assert!(decoded.is_ok(), "calldata should decode as transferWithAuthorizationCall");

        let decoded = decoded.unwrap();
        assert_eq!(decoded.from, meta.from);
        assert_eq!(decoded.to, meta.to);
        assert_eq!(decoded.value, meta.value);
        assert_eq!(decoded.validAfter, meta.valid_after);
        assert_eq!(decoded.validBefore, meta.valid_before);
        assert_eq!(decoded.nonce, meta.nonce);
        assert_eq!(decoded.signature, meta.signature);
    }

    // -----------------------------------------------------------------------
    // build_hook_call3
    // -----------------------------------------------------------------------

    #[test]
    fn build_hook_call3_maps_fields_correctly() {
        let hook = HookCall {
            target: address!("CcCCccCCccCCccCCccCCccCCccCCccCCccCCccCC"),
            calldata: bytes!("aabbccdd"),
            gas_limit: 100_000,
            allow_failure: true,
        };

        let call3 = build_hook_call3(&hook);
        assert_eq!(call3.target, hook.target);
        assert_eq!(call3.callData, hook.calldata);
        assert!(call3.allowFailure);
    }

    #[test]
    fn build_hook_call3_allow_failure_false() {
        let hook = HookCall {
            target: address!("DdDDddDDddDDddDDddDDddDDddDDddDDddDDddDD"),
            calldata: bytes!("11223344"),
            gas_limit: 50_000,
            allow_failure: false,
        };

        let call3 = build_hook_call3(&hook);
        assert!(!call3.allowFailure);
    }

    // -----------------------------------------------------------------------
    // Multicall3 aggregate3 encoding
    // -----------------------------------------------------------------------

    #[test]
    fn aggregate3_encoding_single_transfer() {
        let meta = sample_metadata();
        let call3 = build_transfer_call3(&meta, false);

        let aggregate = IMulticall3::aggregate3Call {
            calls: vec![call3],
        };
        let encoded = aggregate.abi_encode();

        // Should have the aggregate3 selector (4 bytes) plus encoded data
        assert!(encoded.len() > 4);
        // Reasonable size: selector(4) + offset(32) + length(32) + at least one Call3 struct
        assert!(encoded.len() > 100, "encoded aggregate3 should be a reasonable size, got {} bytes", encoded.len());
    }

    #[test]
    fn aggregate3_encoding_multiple_calls() {
        let meta = sample_metadata();
        let transfer_call = build_transfer_call3(&meta, true);

        let hook = HookCall {
            target: address!("CcCCccCCccCCccCCccCCccCCccCCccCCccCCccCC"),
            calldata: bytes!("aabbccdd"),
            gas_limit: 100_000,
            allow_failure: true,
        };
        let hook_call = build_hook_call3(&hook);

        let aggregate = IMulticall3::aggregate3Call {
            calls: vec![transfer_call, hook_call],
        };
        let encoded = aggregate.abi_encode();

        // With two calls, encoded data should be larger than with one
        let single_meta = sample_metadata();
        let single_call = build_transfer_call3(&single_meta, true);
        let single_aggregate = IMulticall3::aggregate3Call {
            calls: vec![single_call],
        };
        let single_encoded = single_aggregate.abi_encode();

        assert!(
            encoded.len() > single_encoded.len(),
            "two-call encoding ({} bytes) should be larger than single-call ({} bytes)",
            encoded.len(),
            single_encoded.len()
        );
    }

    #[test]
    fn aggregate3_encoding_roundtrip() {
        let meta = sample_metadata();
        let call3 = build_transfer_call3(&meta, false);
        let target = call3.target;
        let allow_failure = call3.allowFailure;
        let calldata_len = call3.callData.len();

        let aggregate = IMulticall3::aggregate3Call {
            calls: vec![call3],
        };
        let encoded = aggregate.abi_encode();

        // Decode and verify
        let decoded = IMulticall3::aggregate3Call::abi_decode(&encoded)
            .expect("should decode aggregate3 calldata");
        assert_eq!(decoded.calls.len(), 1);
        assert_eq!(decoded.calls[0].target, target);
        assert_eq!(decoded.calls[0].allowFailure, allow_failure);
        assert_eq!(decoded.calls[0].callData.len(), calldata_len);
    }
}
