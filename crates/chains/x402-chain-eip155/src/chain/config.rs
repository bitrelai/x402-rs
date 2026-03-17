use alloy_primitives::B256;
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use url::Url;
use x402_types::chain::ChainId;
use x402_types::config::LiteralOrEnv;

use crate::chain::Eip155ChainReference;

/// Configuration for an EVM-compatible chain in the x402 facilitator.
///
/// This struct combines a chain reference with chain-specific configuration
/// including RPC endpoints, signers, and network capabilities.
///
/// # Example
///
/// ```ignore
/// use x402_chain_eip155::chain::{Eip155ChainConfig, Eip155ChainReference};
///
/// let config = Eip155ChainConfig {
///     chain_reference: Eip155ChainReference::new(8453), // Base
///     inner: config_inner,
/// };
/// ```
#[derive(Debug, Clone)]
pub struct Eip155ChainConfig {
    /// The numeric chain ID for this EVM network.
    pub chain_reference: Eip155ChainReference,
    /// Chain-specific configuration details.
    pub inner: Eip155ChainConfigInner,
}

impl Eip155ChainConfig {
    /// Returns the CAIP-2 chain ID for this configuration.
    pub fn chain_id(&self) -> ChainId {
        self.chain_reference.into()
    }
    /// Returns whether this chain supports EIP-1559 gas pricing.
    pub fn eip1559(&self) -> bool {
        self.inner.eip1559
    }

    /// Returns whether this chain supports flashblocks (immediate block finality).
    pub fn flashblocks(&self) -> bool {
        self.inner.flashblocks
    }

    /// Returns the transaction receipt timeout in seconds.
    pub fn receipt_timeout_secs(&self) -> u64 {
        self.inner.receipt_timeout_secs
    }

    /// Returns the signer configuration for this chain.
    pub fn signers(&self) -> &Eip155SignersConfig {
        &self.inner.signers
    }

    /// Returns the RPC endpoint configurations for this chain.
    pub fn rpc(&self) -> &Vec<RpcConfig> {
        &self.inner.rpc
    }

    /// Returns whether to use ordered-failover transport.
    pub fn ordered_fallback(&self) -> Option<bool> {
        self.inner.ordered_fallback
    }

    /// Returns the configured poll interval in milliseconds, if set.
    pub fn poll_interval_ms(&self) -> Option<u64> {
        self.inner.poll_interval_ms
    }

    /// Returns the numeric chain reference.
    pub fn chain_reference(&self) -> Eip155ChainReference {
        self.chain_reference
    }
}

/// Configuration specific to EVM-compatible chains.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Eip155ChainConfigInner {
    /// Whether the chain supports EIP-1559 gas pricing.
    #[serde(default = "eip155_chain_config::default_eip1559")]
    pub eip1559: bool,
    /// Whether the chain supports flashblocks.
    #[serde(default = "eip155_chain_config::default_flashblocks")]
    pub flashblocks: bool,
    /// Signer configuration for this chain (required).
    /// Array of private keys (hex format) or env var references.
    pub signers: Eip155SignersConfig,
    /// RPC provider configuration for this chain (required).
    pub rpc: Vec<RpcConfig>,
    /// How long to wait till the transaction receipt is available (optional)
    #[serde(default = "eip155_chain_config::default_receipt_timeout_secs")]
    pub receipt_timeout_secs: u64,
    /// Whether to use ordered-failover transport instead of the default score-based fallback.
    #[serde(default)]
    pub ordered_fallback: Option<bool>,
    /// Override the receipt poll interval in milliseconds.
    /// Alloy's default is 7000ms for remote RPCs. When flashblocks is true,
    /// defaults to 200ms. Set explicitly to override for any chain.
    #[serde(default)]
    pub poll_interval_ms: Option<u64>,
}

mod eip155_chain_config {
    pub fn default_eip1559() -> bool {
        true
    }
    pub fn default_flashblocks() -> bool {
        false
    }
    pub fn default_receipt_timeout_secs() -> u64 {
        30
    }
}

/// RPC provider configuration for a single provider.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RpcConfig {
    /// HTTP URL for the RPC endpoint.
    pub http: LiteralOrEnv<Url>,
    /// Rate limit for requests per second (optional).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate_limit: Option<u32>,
}

/// Configuration for EVM signers.
///
/// Deserializes an array of private key strings (hex format, 0x-prefixed) and
/// validates them as valid 32-byte private keys. The `EthereumWallet` is created
/// lazily when needed via the `wallet()` method.
///
/// Each string can be:
/// - A literal hex private key: `"0xcafe..."`
/// - An environment variable reference: `"$PRIVATE_KEY"` or `"${PRIVATE_KEY}"`
///
/// Example JSON:
/// ```json
/// {
///   "signers": [
///     "$HOT_WALLET_KEY",
///     "0xcafe000000000000000000000000000000000000000000000000000000000001"
///   ]
/// }
/// ```
pub type Eip155SignersConfig = Vec<LiteralOrEnv<EvmPrivateKey>>;

// ============================================================================
// EVM Private Key
// ============================================================================

/// A validated EVM private key (32 bytes).
///
/// This type represents a raw private key that has been validated as a proper
/// 32-byte hex value. It can be converted to a `PrivateKeySigner` when needed.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct EvmPrivateKey(B256);

impl EvmPrivateKey {
    /// Get the raw 32 bytes of the private key.
    pub fn as_bytes(&self) -> &[u8; 32] {
        self.0.as_ref()
    }
}

impl PartialEq for EvmPrivateKey {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl FromStr for EvmPrivateKey {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        B256::from_str(s)
            .map(Self)
            .map_err(|e| format!("Invalid evm private key: {}", e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A valid 32-byte hex private key for test purposes.
    const TEST_KEY: &str =
        "0xcafe000000000000000000000000000000000000000000000000000000000001";

    /// Build a minimal JSON config with the given ordered_fallback value.
    /// If `ordered_fallback` is None, the field is omitted entirely.
    fn make_config_json(ordered_fallback: Option<bool>) -> String {
        let of_field = match ordered_fallback {
            Some(val) => format!(r#", "ordered_fallback": {}"#, val),
            None => String::new(),
        };

        format!(
            r#"{{
                "signers": ["{}"],
                "rpc": [
                    {{ "http": "https://rpc.example.com" }}
                ]{}
            }}"#,
            TEST_KEY, of_field
        )
    }

    // -----------------------------------------------------------------------
    // ordered_fallback: defaults to None when not present
    // -----------------------------------------------------------------------

    #[test]
    fn ordered_fallback_defaults_to_none() {
        let json = make_config_json(None);
        let config: Eip155ChainConfigInner = serde_json::from_str(&json).unwrap();
        assert_eq!(config.ordered_fallback, None);
    }

    // -----------------------------------------------------------------------
    // ordered_fallback: parses as Some(true)
    // -----------------------------------------------------------------------

    #[test]
    fn ordered_fallback_parses_true() {
        let json = make_config_json(Some(true));
        let config: Eip155ChainConfigInner = serde_json::from_str(&json).unwrap();
        assert_eq!(config.ordered_fallback, Some(true));
    }

    // -----------------------------------------------------------------------
    // ordered_fallback: parses as Some(false)
    // -----------------------------------------------------------------------

    #[test]
    fn ordered_fallback_parses_false() {
        let json = make_config_json(Some(false));
        let config: Eip155ChainConfigInner = serde_json::from_str(&json).unwrap();
        assert_eq!(config.ordered_fallback, Some(false));
    }

    // -----------------------------------------------------------------------
    // Other defaults: eip1559, flashblocks, receipt_timeout_secs
    // -----------------------------------------------------------------------

    #[test]
    fn default_eip1559_is_true() {
        let json = make_config_json(None);
        let config: Eip155ChainConfigInner = serde_json::from_str(&json).unwrap();
        assert!(config.eip1559);
    }

    #[test]
    fn default_flashblocks_is_false() {
        let json = make_config_json(None);
        let config: Eip155ChainConfigInner = serde_json::from_str(&json).unwrap();
        assert!(!config.flashblocks);
    }

    #[test]
    fn default_receipt_timeout_secs() {
        let json = make_config_json(None);
        let config: Eip155ChainConfigInner = serde_json::from_str(&json).unwrap();
        assert_eq!(config.receipt_timeout_secs, 30);
    }

    // -----------------------------------------------------------------------
    // RPC config parsing
    // -----------------------------------------------------------------------

    #[test]
    fn rpc_config_with_rate_limit() {
        let json = format!(
            r#"{{
                "signers": ["{}"],
                "rpc": [
                    {{ "http": "https://rpc.example.com", "rate_limit": 100 }}
                ]
            }}"#,
            TEST_KEY
        );

        let config: Eip155ChainConfigInner = serde_json::from_str(&json).unwrap();
        assert_eq!(config.rpc.len(), 1);
        assert_eq!(config.rpc[0].rate_limit, Some(100));
    }

    #[test]
    fn rpc_config_without_rate_limit() {
        let json = format!(
            r#"{{
                "signers": ["{}"],
                "rpc": [
                    {{ "http": "https://rpc.example.com" }}
                ]
            }}"#,
            TEST_KEY
        );

        let config: Eip155ChainConfigInner = serde_json::from_str(&json).unwrap();
        assert_eq!(config.rpc[0].rate_limit, None);
    }

    #[test]
    fn multiple_rpc_endpoints() {
        let json = format!(
            r#"{{
                "signers": ["{}"],
                "rpc": [
                    {{ "http": "https://rpc1.example.com" }},
                    {{ "http": "https://rpc2.example.com", "rate_limit": 50 }}
                ]
            }}"#,
            TEST_KEY
        );

        let config: Eip155ChainConfigInner = serde_json::from_str(&json).unwrap();
        assert_eq!(config.rpc.len(), 2);
    }
}
