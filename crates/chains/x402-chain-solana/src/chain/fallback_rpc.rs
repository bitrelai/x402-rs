//! Ordered-failover wrapper for Solana RPC clients.
//!
//! Unlike EVM (Alloy's composable `Service<RequestPacket>` trait), Solana uses
//! `solana_client::nonblocking::rpc_client::RpcClient` — a concrete type with no
//! pluggable transport. We hold multiple `RpcClient` instances and try them in
//! order, falling through on retryable errors.

use solana_client::client_error::{ClientError, ClientErrorKind};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_client::rpc_config::{RpcSendTransactionConfig, RpcSimulateTransactionConfig};
use solana_client::rpc_request::RpcError;
use solana_client::rpc_response::RpcResult;
use solana_commitment_config::CommitmentConfig;
use solana_pubkey::Pubkey;
use solana_signature::Signature;
use solana_transaction::versioned::VersionedTransaction;
use std::time::Duration;

/// Wrapper around one or more `RpcClient` instances that tries them in order.
///
/// On retryable errors (transport failures, rate limits, unhealthy nodes),
/// the next client is tried. On non-retryable errors (transaction errors,
/// signing errors), the error is returned immediately.
pub struct SolanaFallbackClient {
    clients: Vec<RpcClient>,
}

impl SolanaFallbackClient {
    /// Create a new fallback client from a list of RPC endpoint URLs.
    ///
    /// Each `RpcClient` is created with an explicit per-client timeout to avoid
    /// amplification: 2 clients × 10s = 20s worst-case.
    pub fn new(urls: Vec<url::Url>, timeout: Duration) -> Self {
        assert!(!urls.is_empty(), "at least one RPC URL required");
        let clients = urls
            .into_iter()
            .map(|url| RpcClient::new_with_timeout(url.to_string(), timeout))
            .collect();
        Self { clients }
    }

    /// Returns the URL of the primary (first) client, for debug/logging.
    pub fn url(&self) -> String {
        self.clients[0].url()
    }

    /// Fetch multiple accounts, with fallback on retryable errors.
    pub async fn get_multiple_accounts(
        &self,
        pubkeys: &[Pubkey],
    ) -> Result<Vec<Option<solana_account::Account>>, ClientError> {
        let mut last_err = None;
        for (i, client) in self.clients.iter().enumerate() {
            match client.get_multiple_accounts(pubkeys).await {
                Ok(result) => return Ok(result),
                Err(e) => {
                    if !is_retryable(&e) {
                        return Err(e);
                    }
                    #[cfg(feature = "telemetry")]
                    tracing::warn!(transport = i, error = %e, "retryable error in get_multiple_accounts, trying next");
                    #[cfg(not(feature = "telemetry"))]
                    let _ = i;
                    last_err = Some(e);
                }
            }
        }
        Err(last_err.expect("at least one client"))
    }

    /// Simulate a transaction, with fallback on retryable errors.
    pub async fn simulate_transaction_with_config(
        &self,
        tx: &VersionedTransaction,
        cfg: RpcSimulateTransactionConfig,
    ) -> RpcResult<solana_client::rpc_response::RpcSimulateTransactionResult> {
        let mut last_err = None;
        for (i, client) in self.clients.iter().enumerate() {
            match client
                .simulate_transaction_with_config(tx, cfg.clone())
                .await
            {
                Ok(result) => return Ok(result),
                Err(e) => {
                    if !is_retryable(&e) {
                        return Err(e);
                    }
                    #[cfg(feature = "telemetry")]
                    tracing::warn!(transport = i, error = %e, "retryable error in simulate_transaction, trying next");
                    #[cfg(not(feature = "telemetry"))]
                    let _ = i;
                    last_err = Some(e);
                }
            }
        }
        Err(last_err.expect("at least one client"))
    }

    /// Send a transaction, with fallback on retryable errors.
    pub async fn send_transaction_with_config(
        &self,
        tx: &impl solana_client::rpc_client::SerializableTransaction,
        cfg: RpcSendTransactionConfig,
    ) -> Result<Signature, ClientError> {
        let mut last_err = None;
        for (i, client) in self.clients.iter().enumerate() {
            match client.send_transaction_with_config(tx, cfg).await {
                Ok(sig) => return Ok(sig),
                Err(e) => {
                    if !is_retryable(&e) {
                        return Err(e);
                    }
                    #[cfg(feature = "telemetry")]
                    tracing::warn!(transport = i, error = %e, "retryable error in send_transaction, trying next");
                    #[cfg(not(feature = "telemetry"))]
                    let _ = i;
                    last_err = Some(e);
                }
            }
        }
        Err(last_err.expect("at least one client"))
    }

    /// Confirm a transaction, with fallback on retryable errors.
    pub async fn confirm_transaction_with_commitment(
        &self,
        signature: &Signature,
        commitment_config: CommitmentConfig,
    ) -> RpcResult<bool> {
        let mut last_err = None;
        for (i, client) in self.clients.iter().enumerate() {
            match client
                .confirm_transaction_with_commitment(signature, commitment_config)
                .await
            {
                Ok(result) => return Ok(result),
                Err(e) => {
                    if !is_retryable(&e) {
                        return Err(e);
                    }
                    #[cfg(feature = "telemetry")]
                    tracing::warn!(transport = i, error = %e, "retryable error in confirm_transaction, trying next");
                    #[cfg(not(feature = "telemetry"))]
                    let _ = i;
                    last_err = Some(e);
                }
            }
        }
        Err(last_err.expect("at least one client"))
    }
}

/// Classify whether a `ClientError` is retryable (try next client) or terminal.
///
/// Retryable:
/// - `Io` — network/DNS failure
/// - `Reqwest` — HTTP transport failure
/// - `Middleware` — proxy failure
/// - `RpcError(RpcResponseError { code, .. })` — node unhealthy (-32005),
///   behind (-32016), or HTTP 429/502/503
/// - `RpcError(RpcRequestError(msg))` — contains "error sending request"
///
/// Non-retryable (return immediately):
/// - `TransactionError` — on-chain validation failure
/// - `SigningError` — crypto failure
/// - `SerdeJson` — response parse failure
/// - `RpcError(RpcResponseError { code, .. })` — any other RPC error code
/// - `Custom` — application error
fn is_retryable(err: &ClientError) -> bool {
    match err.kind.as_ref() {
        ClientErrorKind::Io(_) => true,
        ClientErrorKind::Reqwest(_) => true,
        ClientErrorKind::Middleware(_) => true,
        ClientErrorKind::RpcError(rpc_err) => match rpc_err {
            RpcError::RpcResponseError { code, .. } => {
                matches!(code, -32005 | -32016 | 429 | 502 | 503)
            }
            RpcError::RpcRequestError(msg) => {
                msg.contains("error sending request") || msg.contains("connection")
            }
            _ => false,
        },
        ClientErrorKind::TransactionError(_) => false,
        ClientErrorKind::SigningError(_) => false,
        ClientErrorKind::SerdeJson(_) => false,
        ClientErrorKind::Custom(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retryable_io_error() {
        let err = ClientError::from(ClientErrorKind::Io(std::io::Error::new(
            std::io::ErrorKind::ConnectionRefused,
            "connection refused",
        )));
        assert!(is_retryable(&err));
    }

    #[test]
    fn retryable_rpc_response_429() {
        let err = ClientError::from(ClientErrorKind::RpcError(RpcError::RpcResponseError {
            code: 429,
            message: "too many requests".to_string(),
            data: solana_client::rpc_request::RpcResponseErrorData::Empty,
        }));
        assert!(is_retryable(&err));
    }

    #[test]
    fn retryable_rpc_response_node_unhealthy() {
        let err = ClientError::from(ClientErrorKind::RpcError(RpcError::RpcResponseError {
            code: -32005,
            message: "node is unhealthy".to_string(),
            data: solana_client::rpc_request::RpcResponseErrorData::Empty,
        }));
        assert!(is_retryable(&err));
    }

    #[test]
    fn retryable_rpc_request_error() {
        let err = ClientError::from(ClientErrorKind::RpcError(RpcError::RpcRequestError(
            "error sending request: connection reset".to_string(),
        )));
        assert!(is_retryable(&err));
    }

    #[test]
    fn non_retryable_transaction_error() {
        use solana_client::rpc_response::TransactionError;
        let err =
            ClientError::from(ClientErrorKind::TransactionError(TransactionError::AccountNotFound));
        assert!(!is_retryable(&err));
    }

    #[test]
    fn non_retryable_custom() {
        let err = ClientError::from(ClientErrorKind::Custom("application error".to_string()));
        assert!(!is_retryable(&err));
    }

    #[test]
    fn non_retryable_rpc_response_other_code() {
        let err = ClientError::from(ClientErrorKind::RpcError(RpcError::RpcResponseError {
            code: -32600,
            message: "invalid request".to_string(),
            data: solana_client::rpc_request::RpcResponseErrorData::Empty,
        }));
        assert!(!is_retryable(&err));
    }
}
