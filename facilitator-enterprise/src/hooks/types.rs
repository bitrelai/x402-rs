use alloy::primitives::{Address, Bytes, FixedBytes, U256};

/// Settlement metadata extracted from SettleRequest payload.
/// Replaces infra402's crate::chain::evm::SettlementMetadata.
#[derive(Debug, Clone)]
pub struct SettlementMetadata {
    pub from: Address,
    pub to: Address,
    pub value: U256,
    pub valid_after: U256,
    pub valid_before: U256,
    pub nonce: FixedBytes<32>,
    pub signature: Bytes,
    pub contract_address: Address,
}
