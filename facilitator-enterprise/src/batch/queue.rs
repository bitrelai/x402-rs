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

                queue_clone.task_running.store(false, Ordering::SeqCst);
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

    #[cfg(feature = "chain-eip155")]
    async fn process_loop(
        &self,
        provider: &Arc<Eip155ChainProvider>,
        hook_manager: Option<&Arc<HookManager>>,
        allow_partial_failure: bool,
    ) {
        // Wait for requests to accumulate
        let mut ticker = interval(Duration::from_millis(self.max_wait_ms));
        ticker.tick().await; // first tick is immediate
        ticker.tick().await; // second tick waits max_wait_ms

        // Flush batch
        let batch = {
            let mut pending = self.pending.lock().await;
            if pending.is_empty() {
                return;
            }
            let batch_size = std::cmp::min(pending.len(), self.max_batch_size);
            pending.drain(..batch_size).collect::<Vec<_>>()
        };

        if batch.is_empty() {
            return;
        }

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
    }
}
