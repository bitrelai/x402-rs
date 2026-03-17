use alloy::primitives::{Address, Bytes, U256, b256};
use alloy::sol_types::SolCall;
use std::sync::Arc;

use super::multicall3::{self, IMulticall3, MULTICALL3_ADDRESS};
use crate::hooks::types::SettlementMetadata;
use crate::hooks::{HookCall, HookManager, RuntimeContext};

#[cfg(feature = "chain-eip155")]
use x402_chain_eip155::chain::{Eip155ChainProvider, Eip155MetaTransactionProvider, MetaTransaction};

/// ERC-20 Transfer event signature: keccak256("Transfer(address,address,uint256)")
const TRANSFER_EVENT_SIGNATURE: alloy::primitives::B256 =
    b256!("ddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef");

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

/// Parse ERC-20 Transfer event logs from a receipt to determine per-settlement success.
///
/// Each successful `transferWithAuthorization` emits a `Transfer(from, to, value)` event.
/// We match these against the settlement metadata to determine which settlements succeeded.
/// When `allow_partial_failure` is true, some settlements may fail without reverting the batch.
#[cfg(feature = "chain-eip155")]
fn parse_per_settlement_results(
    receipt: &alloy::rpc::types::TransactionReceipt,
    entries: &[&BatchEntry],
) -> Vec<bool> {
    if !receipt.status() {
        // Entire transaction reverted — all settlements failed
        return vec![false; entries.len()];
    }

    // Parse Transfer events from logs
    let mut transfer_events: Vec<(Address, Address, U256)> = Vec::new();
    if let Some(receipt_inner) = receipt.inner.as_receipt() {
        for log in &receipt_inner.logs {
            if log.topics().len() >= 3 && log.topics()[0] == TRANSFER_EVENT_SIGNATURE {
                let from = Address::from_word(log.topics()[1]);
                let to = Address::from_word(log.topics()[2]);
                let value = if log.data().data.len() >= 32 {
                    U256::from_be_slice(&log.data().data[..32])
                } else {
                    U256::ZERO
                };
                transfer_events.push((from, to, value));
            }
        }
    }

    tracing::debug!(
        transfer_count = transfer_events.len(),
        expected_count = entries.len(),
        "parsed Transfer events from batch settlement receipt"
    );

    // Match each settlement to a Transfer event by (from, to, value)
    entries
        .iter()
        .map(|entry| {
            transfer_events.iter().any(|(from, to, value)| {
                *from == entry.metadata.from
                    && *to == entry.metadata.to
                    && *value == entry.metadata.value
            })
        })
        .collect()
}

/// Maximum Call3 structs per Multicall3 transaction.
/// Each settlement needs 1 Call3 for the transfer + N for hooks.
/// Limited by block gas limit (~30M gas / ~55k per transfer ≈ 545 theoretical max).
/// Conservative default matching infra402.
const MAX_CALL3_PER_BATCH: usize = 150;

/// A prepared settlement with its Call3 entries and the original BatchEntry.
struct PreparedSettlement {
    calls: Vec<IMulticall3::Call3>,
    entry_index: usize,
}

/// Process a batch of settlements via Multicall3.
///
/// 1. Builds Call3 entries per settlement (transfer + hooks)
/// 2. Splits into sub-batches when Call3 count exceeds MAX_CALL3_PER_BATCH
/// 3. Submits each sub-batch as a separate Multicall3 transaction
/// 4. Parses per-settlement results from ERC-20 Transfer event logs
/// 5. Routes results back to callers via oneshot channels
///
/// Uses real block context for hook runtime parameters.
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

    // Get the first signer address for runtime context
    let signer_addresses = provider.signer_addresses();
    let sender = signer_addresses
        .first()
        .and_then(|s| s.parse::<Address>().ok())
        .unwrap_or(Address::ZERO);

    // Fetch real block context for hook runtime parameters
    let runtime = match RuntimeContext::from_provider(provider.inner(), sender).await {
        Ok(ctx) => ctx,
        Err(e) => {
            tracing::warn!(error = %e, "Failed to fetch block context for hooks, using zeros");
            RuntimeContext::new(U256::ZERO, U256::ZERO, sender)
        }
    };

    // Prepare Call3 entries for each settlement
    let mut prepared: Vec<PreparedSettlement> = Vec::with_capacity(entries.len());
    for (idx, entry) in entries.iter().enumerate() {
        let mut calls = Vec::new();

        // Transfer Call3
        calls.push(build_transfer_call3(&entry.metadata, allow_partial_failure));

        // Hook Call3s
        if let Some(hm) = hook_manager {
            let hook_runtime = runtime.clone().with_batch_info(idx, entries.len());

            match hm
                .get_hooks_for_destination_with_context(
                    entry.metadata.to,
                    entry.metadata.contract_address,
                    &entry.network,
                    &entry.metadata,
                    &hook_runtime,
                )
                .await
            {
                Ok(hooks) => {
                    for hook in &hooks {
                        calls.push(build_hook_call3(hook));
                    }
                }
                Err(e) => {
                    tracing::error!(error = %e, "Failed to resolve hooks for batch entry");
                    return Err(BatchError::Hook(e.to_string()));
                }
            }
        }

        prepared.push(PreparedSettlement {
            calls,
            entry_index: idx,
        });
    }

    // Split into sub-batches based on Call3 count (matching infra402 logic).
    // Each settlement's calls (transfer + hooks) stay together — never split across batches.
    let mut sub_batches: Vec<Vec<usize>> = Vec::new(); // indices into `prepared`
    let mut current_batch: Vec<usize> = Vec::new();
    let mut current_call3_count = 0;

    for (i, p) in prepared.iter().enumerate() {
        let calls_needed = p.calls.len();

        if current_call3_count + calls_needed > MAX_CALL3_PER_BATCH && !current_batch.is_empty() {
            sub_batches.push(current_batch);
            current_batch = Vec::new();
            current_call3_count = 0;
        }

        current_batch.push(i);
        current_call3_count += calls_needed;
    }
    if !current_batch.is_empty() {
        sub_batches.push(current_batch);
    }

    tracing::info!(
        total_entries = entries.len(),
        sub_batch_count = sub_batches.len(),
        "Split batch into {} sub-batches based on Call3 limits",
        sub_batches.len()
    );

    // Regroup entries into sub-batches for sequential processing.
    // Each sub-batch is a Vec of (BatchEntry, Vec<Call3>).
    let mut entries_with_calls: Vec<(BatchEntry, Vec<IMulticall3::Call3>)> = entries
        .into_iter()
        .zip(prepared.into_iter())
        .map(|(entry, p)| (entry, p.calls))
        .collect();

    let mut sub_batch_ranges: Vec<(usize, usize)> = Vec::new(); // (start, len) into entries_with_calls
    {
        let mut start = 0;
        let mut call3_count = 0;
        for (i, (_, calls)) in entries_with_calls.iter().enumerate() {
            let needed = calls.len();
            if call3_count + needed > MAX_CALL3_PER_BATCH && i > start {
                sub_batch_ranges.push((start, i - start));
                start = i;
                call3_count = 0;
            }
            call3_count += needed;
        }
        if start < entries_with_calls.len() {
            sub_batch_ranges.push((start, entries_with_calls.len() - start));
        }
    }

    tracing::info!(
        total_entries = entries_with_calls.len(),
        sub_batch_count = sub_batch_ranges.len(),
        "Split batch into {} sub-batches based on Call3 limits",
        sub_batch_ranges.len()
    );

    // Process sub-batches in reverse order so we can drain from the end without
    // invalidating indices. Actually, simpler: drain the whole vec into sub-batch vecs.
    let mut sub_batches: Vec<Vec<(BatchEntry, Vec<IMulticall3::Call3>)>> = Vec::new();
    for &(start, len) in sub_batch_ranges.iter().rev() {
        let sub: Vec<_> = entries_with_calls.drain(start..start + len).collect();
        sub_batches.push(sub);
    }
    sub_batches.reverse();

    let mut last_tx_hash = String::new();

    for (batch_num, sub_batch) in sub_batches.into_iter().enumerate() {
        // Collect all Call3s for this sub-batch
        let mut all_calls: Vec<IMulticall3::Call3> = Vec::new();
        for (_, calls) in &sub_batch {
            all_calls.extend(calls.iter().cloned());
        }

        let total_call3s = all_calls.len();
        tracing::info!(
            batch_num,
            settlements = sub_batch.len(),
            total_call3s,
            "Processing sub-batch"
        );

        // Encode and submit
        let aggregate_call = IMulticall3::aggregate3Call { calls: all_calls };
        let calldata = Bytes::from(aggregate_call.abi_encode());
        let meta_tx = MetaTransaction::new(MULTICALL3_ADDRESS, calldata);

        match provider.send_transaction(meta_tx).await {
            Ok(receipt) => {
                let tx_hash = format!("{:?}", receipt.transaction_hash);

                tracing::info!(
                    tx_hash = %tx_hash,
                    success = receipt.status(),
                    settlements = sub_batch.len(),
                    "Sub-batch transaction submitted"
                );

                // Parse per-settlement results from Transfer event logs
                // We need BatchEntry refs for the metadata matching
                let dummy_entries: Vec<BatchEntry> = sub_batch
                    .iter()
                    .map(|(e, _)| BatchEntry {
                        metadata: e.metadata.clone(),
                        network: e.network.clone(),
                        response_tx: tokio::sync::oneshot::channel().0, // dummy
                    })
                    .collect();
                let entry_refs: Vec<&BatchEntry> = dummy_entries.iter().collect();
                let per_success = parse_per_settlement_results(&receipt, &entry_refs);

                // Route results back to callers
                for (pos, (entry, _)) in sub_batch.into_iter().enumerate() {
                    let success = per_success.get(pos).copied().unwrap_or(false);
                    let result = BatchSettleResult {
                        success,
                        tx_hash: tx_hash.clone(),
                    };
                    let _ = entry.response_tx.send(Ok(result));
                }

                last_tx_hash = tx_hash;
            }
            Err(e) => {
                tracing::error!(error = %e, batch_num, "Sub-batch transaction failed");

                // Send error to all entries in this sub-batch
                for (entry, _) in sub_batch {
                    let _ = entry.response_tx.send(Err(BatchError::Transaction(
                        format!("Sub-batch {} failed: {}", batch_num, e),
                    )));
                }

                return Err(BatchError::Transaction(e.to_string()));
            }
        }
    }

    Ok(last_tx_hash)
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

        assert!(call3.callData.len() > 4);

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

    #[test]
    fn aggregate3_encoding_single_transfer() {
        let meta = sample_metadata();
        let call3 = build_transfer_call3(&meta, false);

        let aggregate = IMulticall3::aggregate3Call {
            calls: vec![call3],
        };
        let encoded = aggregate.abi_encode();

        assert!(encoded.len() > 4);
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

        let decoded = IMulticall3::aggregate3Call::abi_decode(&encoded)
            .expect("should decode aggregate3 calldata");
        assert_eq!(decoded.calls.len(), 1);
        assert_eq!(decoded.calls[0].target, target);
        assert_eq!(decoded.calls[0].allowFailure, allow_failure);
        assert_eq!(decoded.calls[0].callData.len(), calldata_len);
    }
}
