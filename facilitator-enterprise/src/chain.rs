use std::collections::HashMap;
#[cfg(any(
    feature = "chain-aptos",
    feature = "chain-eip155",
    feature = "chain-solana"
))]
use std::sync::Arc;
#[cfg(feature = "chain-aptos")]
use x402_chain_aptos::chain as aptos;
#[cfg(feature = "chain-eip155")]
use x402_chain_eip155::chain as eip155;
#[cfg(feature = "chain-solana")]
use x402_chain_solana::chain as solana;
use x402_types::chain::{ChainId, ChainProviderOps, ChainRegistry, FromConfig};

use crate::config::{ChainConfig, ChainsConfig};

#[derive(Debug, Clone)]
pub enum ChainProvider {
    #[cfg(feature = "chain-eip155")]
    Eip155(Arc<eip155::Eip155ChainProvider>),
    #[cfg(feature = "chain-solana")]
    Solana(Arc<solana::SolanaChainProvider>),
    #[cfg(feature = "chain-aptos")]
    Aptos(Arc<aptos::AptosChainProvider>),
}

#[async_trait::async_trait]
impl FromConfig<ChainConfig> for ChainProvider {
    async fn from_config(chains: &ChainConfig) -> Result<Self, Box<dyn std::error::Error>> {
        #[allow(unused_variables)]
        let provider = match chains {
            #[cfg(feature = "chain-eip155")]
            ChainConfig::Eip155(config) => {
                let provider = eip155::Eip155ChainProvider::from_config(config).await?;
                ChainProvider::Eip155(Arc::new(provider))
            }
            #[cfg(feature = "chain-solana")]
            ChainConfig::Solana(config) => {
                let provider = solana::SolanaChainProvider::from_config(config).await?;
                ChainProvider::Solana(Arc::new(provider))
            }
            #[cfg(feature = "chain-aptos")]
            ChainConfig::Aptos(config) => {
                let provider = aptos::AptosChainProvider::from_config(config).await?;
                ChainProvider::Aptos(Arc::new(provider))
            }
            #[allow(unreachable_patterns)]
            _ => unreachable!("ChainConfig variant not enabled in this build"),
        };
        #[allow(unreachable_code)]
        Ok(provider)
    }
}

impl ChainProviderOps for ChainProvider {
    fn signer_addresses(&self) -> Vec<String> {
        match self {
            #[cfg(feature = "chain-eip155")]
            ChainProvider::Eip155(provider) => provider.signer_addresses(),
            #[cfg(feature = "chain-solana")]
            ChainProvider::Solana(provider) => provider.signer_addresses(),
            #[cfg(feature = "chain-aptos")]
            ChainProvider::Aptos(provider) => provider.signer_addresses(),
            #[allow(unreachable_patterns)]
            _ => unreachable!("ChainProvider variant not enabled in this build"),
        }
    }

    fn chain_id(&self) -> ChainId {
        match self {
            #[cfg(feature = "chain-eip155")]
            ChainProvider::Eip155(provider) => provider.chain_id(),
            #[cfg(feature = "chain-solana")]
            ChainProvider::Solana(provider) => provider.chain_id(),
            #[cfg(feature = "chain-aptos")]
            ChainProvider::Aptos(provider) => provider.chain_id(),
            #[allow(unreachable_patterns)]
            _ => unreachable!("ChainProvider variant not enabled in this build"),
        }
    }
}

#[async_trait::async_trait]
impl FromConfig<ChainsConfig> for ChainRegistry<ChainProvider> {
    async fn from_config(chains: &ChainsConfig) -> Result<Self, Box<dyn std::error::Error>> {
        let mut providers = HashMap::new();
        for chain in chains.iter() {
            let chain_provider = ChainProvider::from_config(chain).await?;
            providers.insert(chain_provider.chain_id(), chain_provider);
        }
        Ok(Self::new(providers))
    }
}
