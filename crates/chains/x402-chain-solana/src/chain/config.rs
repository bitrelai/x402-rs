use serde::{Deserialize, Serialize};
use std::ops::Deref;
use std::str::FromStr;
use url::Url;
use x402_types::chain::ChainId;
use x402_types::config::LiteralOrEnv;

use crate::chain::SolanaChainReference;

/// Deserializer that accepts either a single URL string or an array of URL strings.
///
/// This enables backwards-compatible configuration: existing configs with a single
/// `"rpc": "https://..."` string still work, while new configs can specify an array
/// `"rpc": ["https://primary...", "https://fallback..."]` for ordered failover.
fn one_or_many_url<'de, D>(deserializer: D) -> Result<Vec<LiteralOrEnv<Url>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de;

    struct OneOrManyVisitor;

    impl<'de> de::Visitor<'de> for OneOrManyVisitor {
        type Value = Vec<LiteralOrEnv<Url>>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a single URL string or array of URL strings")
        }

        fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
            let url =
                LiteralOrEnv::<Url>::deserialize(serde::de::value::StrDeserializer::new(v))?;
            Ok(vec![url])
        }

        fn visit_seq<S: de::SeqAccess<'de>>(self, mut seq: S) -> Result<Self::Value, S::Error> {
            let mut urls = Vec::new();
            while let Some(url) = seq.next_element::<LiteralOrEnv<Url>>()? {
                urls.push(url);
            }
            if urls.is_empty() {
                return Err(de::Error::custom("rpc array must not be empty"));
            }
            Ok(urls)
        }
    }

    deserializer.deserialize_any(OneOrManyVisitor)
}

/// Serializer that writes a single URL as a plain string, and multiple URLs as an array.
/// This preserves backwards compatibility for configs that only have one RPC endpoint.
fn serialize_one_or_many_url<S>(urls: &[LiteralOrEnv<Url>], serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    if urls.len() == 1 {
        urls[0].serialize(serializer)
    } else {
        urls.serialize(serializer)
    }
}

/// Configuration for a Solana chain in the x402 facilitator.
///
/// This struct combines a chain reference with chain-specific configuration
/// including RPC endpoints, signer, and compute budget parameters.
///
/// # Example
///
/// ```ignore
/// use x402_chain_solana::chain::{SolanaChainConfig, SolanaChainReference};
///
/// let config = SolanaChainConfig {
///     chain_reference: SolanaChainReference::solana(),
///     inner: config_inner,
/// };
/// ```
#[derive(Debug, Clone)]
pub struct SolanaChainConfig {
    /// The Solana network identifier (genesis hash prefix).
    pub chain_reference: SolanaChainReference,
    /// Chain-specific configuration details.
    pub inner: SolanaChainConfigInner,
}

impl SolanaChainConfig {
    /// Returns the signer configuration for this chain.
    pub fn signer(&self) -> &SolanaSignerConfig {
        &self.inner.signer
    }
    /// Returns the primary RPC endpoint URL for this chain (first in the list).
    pub fn rpc(&self) -> &Url {
        &self.inner.rpc[0]
    }

    /// Returns all configured RPC endpoint URLs.
    ///
    /// When multiple URLs are configured, they are used for ordered fallback:
    /// the first URL is the primary, subsequent URLs are tried on retryable errors.
    pub fn rpc_urls(&self) -> &[LiteralOrEnv<Url>] {
        &self.inner.rpc
    }

    /// Returns the maximum compute unit limit for transactions.
    pub fn max_compute_unit_limit(&self) -> u32 {
        self.inner.max_compute_unit_limit
    }

    /// Returns the maximum compute unit price (in micro-lamports).
    pub fn max_compute_unit_price(&self) -> u64 {
        self.inner.max_compute_unit_price
    }

    /// Returns the chain reference (genesis hash prefix).
    pub fn chain_reference(&self) -> SolanaChainReference {
        self.chain_reference
    }

    /// Returns the CAIP-2 chain ID for this configuration.
    pub fn chain_id(&self) -> ChainId {
        self.chain_reference.into()
    }

    /// Returns the optional WebSocket pubsub endpoint URL.
    pub fn pubsub(&self) -> Option<&Url> {
        self.inner.pubsub.as_deref()
    }
}

/// Configuration specific to Solana chains.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SolanaChainConfigInner {
    /// Signer configuration for this chain (required).
    /// A single private key (base58 format, 64 bytes) or env var reference.
    pub signer: SolanaSignerConfig,
    /// RPC provider endpoint(s). Supports a single URL string or an array of URL strings.
    /// When multiple URLs are provided, ordered fallback is used.
    #[serde(deserialize_with = "one_or_many_url", serialize_with = "serialize_one_or_many_url")]
    pub rpc: Vec<LiteralOrEnv<Url>>,
    /// RPC pubsub provider endpoint (optional)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pubsub: Option<LiteralOrEnv<Url>>,
    /// Maximum compute unit limit for transactions (optional)
    #[serde(default = "solana_chain_config::default_max_compute_unit_limit")]
    pub max_compute_unit_limit: u32,
    /// Maximum compute unit price for transactions (optional)
    #[serde(default = "solana_chain_config::default_max_compute_unit_price")]
    pub max_compute_unit_price: u64,
}

mod solana_chain_config {
    pub fn default_max_compute_unit_limit() -> u32 {
        400_000
    }
    pub fn default_max_compute_unit_price() -> u64 {
        1_000_000
    }
}

// ============================================================================
// Solana Private Key
// ============================================================================

/// A validated Solana private key (64 bytes in standard Solana format).
///
/// This type represents a standard Solana keypair in its 64-byte format:
/// - First 32 bytes: the Ed25519 secret key (seed)
/// - Last 32 bytes: the Ed25519 public key
///
/// The key is stored and parsed as a base58-encoded 64-byte array,
/// which is the standard format used by Solana CLI and wallets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SolanaPrivateKey([u8; 64]);

impl SolanaPrivateKey {
    /// Parse a base58 string into a private key (64 bytes in standard Solana format).
    ///
    /// The standard Solana keypair format is 64 bytes:
    /// - First 32 bytes: secret key (seed)
    /// - Last 32 bytes: public key
    pub fn from_base58(s: &str) -> Result<Self, String> {
        let bytes = bs58::decode(s)
            .into_vec()
            .map_err(|e| format!("Invalid base58: {}", e))?;

        if bytes.len() != 64 {
            return Err(format!(
                "Private key must be 64 bytes (standard Solana format), got {} bytes",
                bytes.len()
            ));
        }

        let mut arr = [0u8; 64];
        arr.copy_from_slice(&bytes);
        Ok(Self(arr))
    }

    /// Encode the keypair back to base58.
    pub fn to_base58(&self) -> String {
        bs58::encode(&self.0).into_string()
    }
}

impl Serialize for SolanaPrivateKey {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_base58())
    }
}

impl FromStr for SolanaPrivateKey {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_base58(s)
    }
}

impl std::fmt::Display for SolanaPrivateKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_base58())
    }
}

/// Type alias for Solana signer configuration.
///
/// Uses `LiteralOrEnv` to support both literal base58 keys and environment variable references.
///
/// Example JSON:
/// ```json
/// {
///   "signer": "$SOLANA_FACILITATOR_KEY"
/// }
/// ```
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SolanaSignerConfig(LiteralOrEnv<SolanaPrivateKey>);

impl Deref for SolanaSignerConfig {
    type Target = SolanaPrivateKey;

    fn deref(&self) -> &Self::Target {
        self.0.inner()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Generate a valid base58-encoded 64-byte Solana keypair for test configs.
    fn test_keypair_base58() -> String {
        // Use solana_keypair to generate a valid keypair for testing
        let keypair = solana_keypair::Keypair::new();
        bs58::encode(keypair.to_bytes()).into_string()
    }

    // -----------------------------------------------------------------------
    // one_or_many_url: single URL string deserializes to Vec with 1 element
    // -----------------------------------------------------------------------

    #[test]
    fn one_or_many_url_single_string() {
        let key = test_keypair_base58();
        let json = format!(
            r#"{{
                "signer": "{}",
                "rpc": "https://api.mainnet-beta.solana.com"
            }}"#,
            key
        );

        let config: SolanaChainConfigInner = serde_json::from_str(&json).unwrap();
        assert_eq!(config.rpc.len(), 1);
        assert_eq!(
            config.rpc[0].as_str(),
            "https://api.mainnet-beta.solana.com/"
        );
    }

    // -----------------------------------------------------------------------
    // one_or_many_url: array of URLs deserializes to Vec with multiple elements
    // -----------------------------------------------------------------------

    #[test]
    fn one_or_many_url_array_of_strings() {
        let key = test_keypair_base58();
        let json = format!(
            r#"{{
                "signer": "{}",
                "rpc": [
                    "https://api.mainnet-beta.solana.com",
                    "https://rpc.ankr.com/solana"
                ]
            }}"#,
            key
        );

        let config: SolanaChainConfigInner = serde_json::from_str(&json).unwrap();
        assert_eq!(config.rpc.len(), 2);
    }

    // -----------------------------------------------------------------------
    // one_or_many_url: empty array is rejected
    // -----------------------------------------------------------------------

    #[test]
    fn one_or_many_url_empty_array_rejected() {
        let key = test_keypair_base58();
        let json = format!(
            r#"{{
                "signer": "{}",
                "rpc": []
            }}"#,
            key
        );

        let result = serde_json::from_str::<SolanaChainConfigInner>(&json);
        assert!(result.is_err(), "empty rpc array should be rejected");
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("must not be empty"),
            "error should mention 'must not be empty', got: {}",
            err_msg
        );
    }

    // -----------------------------------------------------------------------
    // Default compute budget values
    // -----------------------------------------------------------------------

    #[test]
    fn default_compute_budget_values() {
        let key = test_keypair_base58();
        let json = format!(
            r#"{{
                "signer": "{}",
                "rpc": "https://api.mainnet-beta.solana.com"
            }}"#,
            key
        );

        let config: SolanaChainConfigInner = serde_json::from_str(&json).unwrap();
        assert_eq!(config.max_compute_unit_limit, 400_000);
        assert_eq!(config.max_compute_unit_price, 1_000_000);
    }

    // -----------------------------------------------------------------------
    // Serialization roundtrip: single URL serializes as plain string
    // -----------------------------------------------------------------------

    #[test]
    fn serialize_single_url_as_string() {
        let key = test_keypair_base58();
        let json = format!(
            r#"{{
                "signer": "{}",
                "rpc": "https://api.mainnet-beta.solana.com"
            }}"#,
            key
        );

        let config: SolanaChainConfigInner = serde_json::from_str(&json).unwrap();
        let serialized = serde_json::to_value(&config).unwrap();

        // Single URL should serialize as a plain string, not an array
        assert!(
            serialized["rpc"].is_string(),
            "single rpc URL should serialize as string, got: {:?}",
            serialized["rpc"]
        );
    }

    #[test]
    fn serialize_multiple_urls_as_array() {
        let key = test_keypair_base58();
        let json = format!(
            r#"{{
                "signer": "{}",
                "rpc": [
                    "https://api.mainnet-beta.solana.com",
                    "https://rpc.ankr.com/solana"
                ]
            }}"#,
            key
        );

        let config: SolanaChainConfigInner = serde_json::from_str(&json).unwrap();
        let serialized = serde_json::to_value(&config).unwrap();

        // Multiple URLs should serialize as an array
        assert!(
            serialized["rpc"].is_array(),
            "multiple rpc URLs should serialize as array, got: {:?}",
            serialized["rpc"]
        );
        assert_eq!(serialized["rpc"].as_array().unwrap().len(), 2);
    }
}
