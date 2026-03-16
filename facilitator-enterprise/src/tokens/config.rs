use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignatureFormat {
    PackedBytes,
    SeparateVrs,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenDefinition {
    pub symbol: String,
    pub name: String,
    pub decimals: u8,
    pub eip712_name: String,
    pub eip712_version: String,
    pub abi_file: String,
    pub signature_format: SignatureFormat,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkTokens {
    #[serde(flatten)]
    pub tokens: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenConfig {
    #[serde(default)]
    pub tokens: TokenSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenSettings {
    #[serde(default)]
    pub custom_tokens_file: Option<String>,
    #[serde(default)]
    pub definitions: HashMap<String, TokenDefinition>,
    #[serde(default)]
    pub networks: HashMap<String, NetworkTokens>,
}

impl Default for TokenSettings {
    fn default() -> Self {
        Self {
            custom_tokens_file: None,
            definitions: HashMap::new(),
            networks: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomTokenConfig {
    #[serde(default)]
    pub tokens: CustomTokenSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CustomTokenSettings {
    #[serde(default)]
    pub definitions: HashMap<String, TokenDefinition>,
    #[serde(default)]
    pub networks: HashMap<String, NetworkTokens>,
}

impl TokenConfig {
    pub fn from_file(path: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let content = std::fs::read_to_string(path)?;
        let mut config: TokenConfig = toml::from_str(&content)?;

        if let Some(ref custom_file) = config.tokens.custom_tokens_file {
            let custom_path = Self::resolve_path(path, custom_file);

            match Self::load_custom_tokens(&custom_path) {
                Ok(custom_config) => {
                    Self::merge_custom_tokens(&mut config.tokens, custom_config)?;
                    tracing::info!(custom_file = custom_path, "Loaded and merged custom tokens");
                }
                Err(e) => {
                    tracing::warn!(
                        custom_file = custom_path,
                        error = %e,
                        "Failed to load custom tokens file (continuing without custom tokens)"
                    );
                }
            }
        }

        tracing::info!(
            path = path,
            definitions_count = config.tokens.definitions.len(),
            networks_count = config.tokens.networks.len(),
            "Loaded token configuration"
        );

        Ok(config)
    }

    fn resolve_path(base_config_path: &str, custom_file: &str) -> String {
        let custom_path = std::path::Path::new(custom_file);

        if custom_path.is_absolute() {
            custom_file.to_string()
        } else {
            if let Some(parent) = std::path::Path::new(base_config_path).parent() {
                parent.join(custom_file).to_string_lossy().to_string()
            } else {
                custom_file.to_string()
            }
        }
    }

    fn load_custom_tokens(path: &str) -> Result<CustomTokenConfig, Box<dyn std::error::Error>> {
        let content = std::fs::read_to_string(path)?;
        let config: CustomTokenConfig = toml::from_str(&content)?;
        Ok(config)
    }

    fn merge_custom_tokens(
        main: &mut TokenSettings,
        custom: CustomTokenConfig,
    ) -> Result<(), Box<dyn std::error::Error>> {
        use alloy::primitives::Address;
        use std::str::FromStr;

        let mut prod_addresses: HashMap<String, HashMap<Address, String>> = HashMap::new();

        for (network_name, network_tokens) in &main.networks {
            let mut network_addrs = HashMap::new();
            for (token_name, address_str) in &network_tokens.tokens {
                if let Ok(addr) = Address::from_str(address_str) {
                    network_addrs.insert(addr, token_name.clone());
                }
            }
            prod_addresses.insert(network_name.clone(), network_addrs);
        }

        for (name, def) in custom.tokens.definitions {
            if main.definitions.contains_key(&name) {
                tracing::warn!(
                    token = name,
                    "Custom token definition overriding production definition"
                );
            }
            main.definitions.insert(name, def);
        }

        for (network_name, custom_network) in custom.tokens.networks {
            let network_tokens = main
                .networks
                .entry(network_name.clone())
                .or_insert_with(|| NetworkTokens {
                    tokens: HashMap::new(),
                });

            for (token_name, address_str) in custom_network.tokens {
                let addr = Address::from_str(&address_str).map_err(|e| {
                    format!(
                        "Invalid address '{}' for custom token '{}': {}",
                        address_str, token_name, e
                    )
                })?;

                if let Some(prod_addrs) = prod_addresses.get(&network_name) {
                    if let Some(prod_token) = prod_addrs.get(&addr) {
                        return Err(format!(
                            "Custom token '{}' at address {} conflicts with production token '{}' on network '{}'",
                            token_name, addr, prod_token, network_name
                        ).into());
                    }
                }

                if network_tokens.tokens.contains_key(&token_name) {
                    tracing::warn!(
                        token = token_name,
                        network = network_name,
                        "Custom token overriding existing token deployment"
                    );
                }

                network_tokens.tokens.insert(token_name, address_str);
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_config_parsing() {
        let toml_str = r#"
[tokens]

[tokens.definitions.usdc]
symbol = "USDC"
name = "USD Coin"
decimals = 6
eip712_name = "USD Coin"
eip712_version = "2"
abi_file = "abi/USDC.json"
signature_format = "packed_bytes"

[tokens.networks.base-sepolia]
usdc = "0x036CbD53842c5426634e7929541eC2318f3dCF7e"
"#;

        let config: TokenConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.tokens.definitions.len(), 1);

        let usdc = config.tokens.definitions.get("usdc").unwrap();
        assert_eq!(usdc.symbol, "USDC");
        assert_eq!(usdc.decimals, 6);
    }
}
