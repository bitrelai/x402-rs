#[allow(unused_imports)]
use crate::chain::ChainProvider;
#[allow(unused_imports)]
use std::sync::Arc;
#[allow(unused_imports)]
use x402_types::scheme::{X402SchemeFacilitator, X402SchemeFacilitatorBuilder};

#[cfg(feature = "chain-aptos")]
use x402_chain_aptos::V2AptosExact;
#[cfg(feature = "chain-eip155")]
use x402_chain_eip155::{V1Eip155Exact, V2Eip155Exact, V2Eip155Upto};
#[cfg(feature = "chain-solana")]
use x402_chain_solana::{V1SolanaExact, V2SolanaExact};

#[cfg(feature = "chain-solana")]
impl X402SchemeFacilitatorBuilder<&ChainProvider> for V1SolanaExact {
    fn build(
        &self,
        provider: &ChainProvider,
        config: Option<serde_json::Value>,
    ) -> Result<Box<dyn X402SchemeFacilitator>, Box<dyn std::error::Error>> {
        #[allow(irrefutable_let_patterns)]
        let solana_provider = if let ChainProvider::Solana(provider) = provider {
            Arc::clone(provider)
        } else {
            return Err("V1SolanaExact::build: provider must be a SolanaChainProvider".into());
        };
        self.build(solana_provider, config)
    }
}

#[cfg(feature = "chain-solana")]
impl X402SchemeFacilitatorBuilder<&ChainProvider> for V2SolanaExact {
    fn build(
        &self,
        provider: &ChainProvider,
        config: Option<serde_json::Value>,
    ) -> Result<Box<dyn X402SchemeFacilitator>, Box<dyn std::error::Error>> {
        #[allow(irrefutable_let_patterns)]
        let solana_provider = if let ChainProvider::Solana(provider) = provider {
            Arc::clone(provider)
        } else {
            return Err("V2SolanaExact::build: provider must be a SolanaChainProvider".into());
        };
        self.build(solana_provider, config)
    }
}

#[cfg(feature = "chain-eip155")]
impl X402SchemeFacilitatorBuilder<&ChainProvider> for V2Eip155Exact {
    fn build(
        &self,
        provider: &ChainProvider,
        config: Option<serde_json::Value>,
    ) -> Result<Box<dyn X402SchemeFacilitator>, Box<dyn std::error::Error>> {
        #[allow(irrefutable_let_patterns)]
        let eip155_provider = if let ChainProvider::Eip155(provider) = provider {
            Arc::clone(provider)
        } else {
            return Err("V2Eip155Exact::build: provider must be an Eip155ChainProvider".into());
        };
        self.build(eip155_provider, config)
    }
}

#[cfg(feature = "chain-eip155")]
impl X402SchemeFacilitatorBuilder<&ChainProvider> for V2Eip155Upto {
    fn build(
        &self,
        provider: &ChainProvider,
        config: Option<serde_json::Value>,
    ) -> Result<Box<dyn X402SchemeFacilitator>, Box<dyn std::error::Error>> {
        #[allow(irrefutable_let_patterns)]
        let eip155_provider = if let ChainProvider::Eip155(provider) = provider {
            Arc::clone(provider)
        } else {
            return Err("V2Eip155Upto::build: provider must be an Eip155ChainProvider".into());
        };
        self.build(eip155_provider, config)
    }
}

#[cfg(feature = "chain-aptos")]
impl X402SchemeFacilitatorBuilder<&ChainProvider> for V2AptosExact {
    fn build(
        &self,
        provider: &ChainProvider,
        config: Option<serde_json::Value>,
    ) -> Result<Box<dyn X402SchemeFacilitator>, Box<dyn std::error::Error>> {
        #[allow(irrefutable_let_patterns)]
        let aptos_provider = if let ChainProvider::Aptos(provider) = provider {
            Arc::clone(provider)
        } else {
            return Err("V2AptosExact::build: provider must be an AptosChainProvider".into());
        };
        self.build(aptos_provider, config)
    }
}

#[cfg(feature = "chain-eip155")]
impl X402SchemeFacilitatorBuilder<&ChainProvider> for V1Eip155Exact {
    fn build(
        &self,
        provider: &ChainProvider,
        config: Option<serde_json::Value>,
    ) -> Result<Box<dyn X402SchemeFacilitator>, Box<dyn std::error::Error>> {
        #[allow(irrefutable_let_patterns)]
        let eip155_provider = if let ChainProvider::Eip155(provider) = provider {
            Arc::clone(provider)
        } else {
            return Err("V1Eip155Exact::build: provider must be an Eip155ChainProvider".into());
        };
        self.build(eip155_provider, config)
    }
}
