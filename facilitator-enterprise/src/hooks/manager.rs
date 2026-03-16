use alloy::primitives::{Address, Bytes};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use super::config::{HookConfig, HookDefinition};
use super::context::RuntimeContext;
use super::errors::HookError;
use super::types::SettlementMetadata;
use crate::tokens::TokenManager;

#[derive(Debug, Clone)]
pub struct HookCall {
    pub target: Address,
    pub calldata: Bytes,
    pub gas_limit: u64,
    pub allow_failure: bool,
}

type HookState = super::config::HookSettings;

#[derive(Debug, Clone)]
pub struct HookManager {
    config_path: String,
    state: Arc<RwLock<HookState>>,
    token_manager: Option<TokenManager>,
}

impl HookManager {
    pub fn new(config_path: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let config = HookConfig::from_file(config_path)?;
        let state = config.hooks;

        tracing::info!(
            path = config_path,
            hooks_count = state.definitions.len(),
            networks_count = state.networks.len(),
            "Initialized HookManager"
        );

        Ok(Self {
            config_path: config_path.to_string(),
            state: Arc::new(RwLock::new(state)),
            token_manager: None,
        })
    }

    pub fn new_with_tokens(
        config_path: &str,
        token_manager: TokenManager,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let config = HookConfig::from_file(config_path)?;
        let state = config.hooks;

        tracing::info!(
            path = config_path,
            hooks_count = state.definitions.len(),
            networks_count = state.networks.len(),
            "Initialized HookManager with token filtering"
        );

        Ok(Self {
            config_path: config_path.to_string(),
            state: Arc::new(RwLock::new(state)),
            token_manager: Some(token_manager),
        })
    }

    pub async fn reload(&self) -> Result<(), String> {
        let config = HookConfig::from_file(&self.config_path).map_err(|e| e.to_string())?;
        let new_state = config.hooks;

        let mut state = self.state.write().await;
        *state = new_state.clone();

        tracing::info!(
            hooks_count = new_state.definitions.len(),
            networks_count = new_state.networks.len(),
            "Reloaded HookManager configuration"
        );

        Ok(())
    }

    pub async fn get_hooks_for_destination_with_context(
        &self,
        destination: Address,
        token_address: Address,
        network: &str,
        metadata: &SettlementMetadata,
        runtime: &RuntimeContext,
    ) -> Result<Vec<HookCall>, HookError> {
        let state = self.state.read().await;

        if !state.is_enabled_for_network(network) {
            return Ok(Vec::new());
        }

        let mappings = state.get_network_mappings(network);

        let mut hook_names: Option<&Vec<String>> = None;
        for (address_str, names) in mappings.iter() {
            if let Some(resolved_addr) =
                super::config::HookSettings::resolve_mapping_address(address_str)
            {
                if resolved_addr == destination {
                    hook_names = Some(names);
                    break;
                }
            }
        }

        let hook_names = match hook_names {
            Some(names) => names,
            None => return Ok(Vec::new()),
        };

        let token_name: Option<String> = if let Some(ref token_mgr) = self.token_manager {
            token_mgr.get_token_name(token_address, network).await
        } else {
            None
        };

        let network_config = state.networks.get(network);

        let mut hooks = Vec::new();
        for name in hook_names {
            if let Some(def) = state.definitions.get(name) {
                if !def.enabled {
                    continue;
                }

                if let (Some(token_name_val), Some(network_cfg)) =
                    (token_name.as_ref(), network_config)
                {
                    if let Some(filter) = network_cfg.token_filters.get(name) {
                        if !filter.matches(token_name_val) {
                            continue;
                        }
                    }
                }

                let contract_address =
                    match state.resolve_contract_address(name, network, &destination) {
                        Some(addr) => addr,
                        None => {
                            tracing::warn!(
                                hook = name,
                                network = network,
                                destination = %destination,
                                "Hook contract address not configured, skipping"
                            );
                            continue;
                        }
                    };

                match def.encode_calldata(metadata, runtime) {
                    Ok(calldata) => {
                        hooks.push(HookCall {
                            target: contract_address,
                            calldata,
                            gas_limit: def.gas_limit,
                            allow_failure: state.allow_hook_failure,
                        });
                    }
                    Err(e) => {
                        tracing::error!(
                            hook = name,
                            error = %e,
                            "Failed to encode hook calldata"
                        );
                        return Err(e);
                    }
                }
            }
        }

        Ok(hooks)
    }

    pub async fn enable_hook(&self, name: &str) -> Result<(), String> {
        let mut state = self.state.write().await;
        match state.definitions.get_mut(name) {
            Some(def) => {
                def.enabled = true;
                Ok(())
            }
            None => Err(format!("Hook '{}' not found", name)),
        }
    }

    pub async fn disable_hook(&self, name: &str) -> Result<(), String> {
        let mut state = self.state.write().await;
        match state.definitions.get_mut(name) {
            Some(def) => {
                def.enabled = false;
                Ok(())
            }
            None => Err(format!("Hook '{}' not found", name)),
        }
    }

    pub async fn get_all_hooks(&self) -> HashMap<String, HookDefinition> {
        let state = self.state.read().await;
        state.definitions.clone()
    }

    pub async fn get_all_mappings(&self) -> HashMap<String, Vec<String>> {
        let state = self.state.read().await;
        state.mappings.clone()
    }

    pub async fn is_enabled(&self) -> bool {
        let state = self.state.read().await;
        state.enabled
    }

    pub async fn get_hook(&self, name: &str) -> Option<HookDefinition> {
        let state = self.state.read().await;
        state.definitions.get(name).cloned()
    }
}
