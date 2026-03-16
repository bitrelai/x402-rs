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
