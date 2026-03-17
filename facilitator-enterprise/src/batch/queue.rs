use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::{Mutex, oneshot};
use tokio::time::{Duration, interval};

use super::processor::{BatchEntry, BatchError, BatchSettleResult};
use crate::enterprise_config::BatchSettlementConfig;
use crate::hooks::HookManager;
use crate::hooks::types::SettlementMetadata;

#[cfg(feature = "chain-eip155")]
use x402_chain_eip155::chain::Eip155ChainProvider;

/// Manages per-network batch queues.
pub struct BatchQueueManager {
    config: BatchSettlementConfig,
    hook_manager: Option<Arc<HookManager>>,
    #[cfg(feature = "chain-eip155")]
    evm_queues: Arc<dashmap::DashMap<String, Arc<BatchQueue>>>,
}

impl BatchQueueManager {
    pub fn new(
        config: BatchSettlementConfig,
        hook_manager: Option<Arc<HookManager>>,
    ) -> Self {
        Self {
            config,
            hook_manager,
            #[cfg(feature = "chain-eip155")]
            evm_queues: Arc::new(dashmap::DashMap::new()),
        }
    }

    /// Check if batch settlement is enabled for the given network.
    pub fn should_batch(&self, network: &str) -> bool {
        self.config.is_enabled_for_network(network)
    }

    /// Enqueue a settlement request for batch processing.
    ///
    /// Returns a oneshot receiver that will contain the result when the batch is processed.
    #[cfg(feature = "chain-eip155")]
    pub async fn enqueue(
        &self,
        network: String,
        metadata: SettlementMetadata,
        provider: Arc<Eip155ChainProvider>,
    ) -> oneshot::Receiver<Result<BatchSettleResult, BatchError>> {
        let (tx, rx) = oneshot::channel();

        let resolved = self.config.for_network(&network);

        let queue = self
            .evm_queues
            .entry(network.clone())
            .or_insert_with(|| {
                Arc::new(BatchQueue::new(
                    resolved.max_batch_size,
                    resolved.max_wait_ms,
                ))
            })
            .clone();

        let entry = BatchEntry {
            metadata,
            network: network.clone(),
            response_tx: tx,
        };

        queue.push(entry).await;

        // Spawn processor if not already running
        if queue
            .task_running
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            let queue_clone = Arc::clone(&queue);
            let provider_clone = Arc::clone(&provider);
            let hook_manager = self.hook_manager.clone();
            let allow_partial = resolved.allow_partial_failure;
            let queues_map = Arc::clone(&self.evm_queues);
            let network_key = network.clone();

            tokio::spawn(async move {
                queue_clone
                    .process_loop(&provider_clone, hook_manager.as_ref(), allow_partial)
                    .await;

                // process_loop clears task_running under the pending lock when the
                // queue is empty, so we do NOT clear it here. We only clean up the
                // DashMap entry.
                queues_map.remove(&network_key);
            });
        }

        rx
    }

    /// Returns the number of active queues.
    pub fn active_queues(&self) -> usize {
        #[cfg(feature = "chain-eip155")]
        {
            self.evm_queues.len()
        }
        #[cfg(not(feature = "chain-eip155"))]
        {
            0
        }
    }
}

/// A single per-network batch queue.
///
/// The process_loop runs until the queue is empty. It clears `task_running`
/// while holding the pending lock, eliminating the race window where an
/// enqueue between drain and flag-clear could leave items without a processor.
struct BatchQueue {
    pending: Mutex<Vec<BatchEntry>>,
    max_batch_size: usize,
    max_wait_ms: u64,
    task_running: AtomicBool,
}

impl BatchQueue {
    fn new(max_batch_size: usize, max_wait_ms: u64) -> Self {
        Self {
            pending: Mutex::new(Vec::new()),
            max_batch_size,
            max_wait_ms,
            task_running: AtomicBool::new(false),
        }
    }

    async fn push(&self, entry: BatchEntry) {
        let mut pending = self.pending.lock().await;
        pending.push(entry);
    }

    /// Run the batch processing loop.
    ///
    /// Waits for max_wait_ms, then flushes. Keeps looping as long as items
    /// remain. Clears `task_running` under the pending lock when the queue
    /// is empty, so no enqueue can sneak in between drain and flag clear.
    #[cfg(feature = "chain-eip155")]
    async fn process_loop(
        &self,
        provider: &Arc<Eip155ChainProvider>,
        hook_manager: Option<&Arc<HookManager>>,
        allow_partial_failure: bool,
    ) {
        // Initial wait for requests to accumulate
        let mut ticker = interval(Duration::from_millis(self.max_wait_ms));
        ticker.tick().await; // first tick is immediate
        ticker.tick().await; // second tick waits max_wait_ms

        loop {
            // Drain under lock. If empty, clear flag and exit — atomically.
            let batch = {
                let mut pending = self.pending.lock().await;
                if pending.is_empty() {
                    // Clear task_running WHILE holding the lock.
                    // Any concurrent enqueue() will see task_running == false
                    // only after we release the lock, and it will then spawn
                    // a new worker.
                    self.task_running.store(false, Ordering::SeqCst);
                    return;
                }
                let batch_size = std::cmp::min(pending.len(), self.max_batch_size);
                pending.drain(..batch_size).collect::<Vec<_>>()
            };
            // Lock released here — new enqueues can happen during processing.

            tracing::info!(batch_size = batch.len(), "Flushing batch settlement");

            if let Err(e) = super::processor::process_batch(
                provider,
                batch,
                hook_manager,
                allow_partial_failure,
            )
            .await
            {
                tracing::error!(error = %e, "Batch settlement failed");
            }

            // Loop back to check if more items arrived during processing.
            // No additional wait — flush immediately if items are pending.
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::enterprise_config::{BatchSettlementConfig, NetworkBatchConfig};
    use std::collections::HashMap;

    fn make_config(
        global_enabled: bool,
        networks: HashMap<String, NetworkBatchConfig>,
    ) -> BatchSettlementConfig {
        BatchSettlementConfig {
            enabled: global_enabled,
            max_batch_size: 150,
            max_wait_ms: 500,
            min_batch_size: 10,
            allow_partial_failure: false,
            allow_hook_failure: false,
            networks,
        }
    }

    #[test]
    fn should_batch_global_enabled_no_overrides() {
        let config = make_config(true, HashMap::new());
        let manager = BatchQueueManager::new(config, None);
        assert!(manager.should_batch("base"));
        assert!(manager.should_batch("polygon"));
        assert!(manager.should_batch("unknown-chain"));
    }

    #[test]
    fn should_batch_global_disabled_no_overrides() {
        let config = make_config(false, HashMap::new());
        let manager = BatchQueueManager::new(config, None);
        assert!(!manager.should_batch("base"));
        assert!(!manager.should_batch("polygon"));
    }

    #[test]
    fn should_batch_global_disabled_network_override_enabled() {
        let mut networks = HashMap::new();
        networks.insert(
            "bsc-testnet".to_string(),
            NetworkBatchConfig {
                enabled: Some(true),
                max_batch_size: Some(100),
                max_wait_ms: None,
                min_batch_size: None,
                allow_partial_failure: None,
            },
        );

        let config = make_config(false, networks);
        let manager = BatchQueueManager::new(config, None);
        assert!(manager.should_batch("bsc-testnet"));
        assert!(!manager.should_batch("base"));
        assert!(!manager.should_batch("polygon"));
    }

    #[test]
    fn should_batch_global_enabled_network_override_disabled() {
        let mut networks = HashMap::new();
        networks.insert(
            "polygon".to_string(),
            NetworkBatchConfig {
                enabled: Some(false),
                max_batch_size: None,
                max_wait_ms: None,
                min_batch_size: None,
                allow_partial_failure: None,
            },
        );

        let config = make_config(true, networks);
        let manager = BatchQueueManager::new(config, None);
        assert!(!manager.should_batch("polygon"));
        assert!(manager.should_batch("base"));
        assert!(manager.should_batch("bsc"));
    }

    #[test]
    fn should_batch_network_override_enabled_none_falls_back_to_global() {
        let mut networks = HashMap::new();
        networks.insert(
            "base".to_string(),
            NetworkBatchConfig {
                enabled: None,
                max_batch_size: Some(200),
                max_wait_ms: None,
                min_batch_size: None,
                allow_partial_failure: None,
            },
        );

        let config = make_config(true, networks);
        let manager = BatchQueueManager::new(config, None);
        assert!(manager.should_batch("base"));
    }

    #[test]
    fn should_batch_network_override_enabled_none_global_disabled() {
        let mut networks = HashMap::new();
        networks.insert(
            "base".to_string(),
            NetworkBatchConfig {
                enabled: None,
                max_batch_size: Some(200),
                max_wait_ms: None,
                min_batch_size: None,
                allow_partial_failure: None,
            },
        );

        let config = make_config(false, networks);
        let manager = BatchQueueManager::new(config, None);
        assert!(!manager.should_batch("base"));
    }

    #[test]
    fn active_queues_starts_at_zero() {
        let config = make_config(true, HashMap::new());
        let manager = BatchQueueManager::new(config, None);
        assert_eq!(manager.active_queues(), 0);
    }
}
