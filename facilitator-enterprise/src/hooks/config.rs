use alloy::dyn_abi::{DynSolType, DynSolValue};
use alloy::primitives::{Address, Bytes, FixedBytes, U256};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::str::FromStr;

use super::context::RuntimeContext;
use super::errors::{HookError, HookResult};
use super::types::SettlementMetadata;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "source_type", content = "field", rename_all = "lowercase")]
pub enum ParameterSource {
    Payment(PaymentField),
    Runtime(RuntimeField),
    Static(String),
    Config(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum PaymentField {
    From,
    To,
    Value,
    ValidAfter,
    ValidBefore,
    Nonce,
    ContractAddress,
    SignatureV,
    SignatureR,
    SignatureS,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeField {
    Timestamp,
    BlockNumber,
    Sender,
    BatchIndex,
    BatchSize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParameterDefinition {
    #[serde(rename = "type")]
    pub sol_type: String,
    pub source: ParameterSource,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookDefinition {
    pub enabled: bool,
    pub function_signature: String,
    #[serde(default)]
    pub parameters: Vec<ParameterDefinition>,
    #[serde(default)]
    pub config_values: HashMap<String, String>,
    #[serde(default)]
    pub gas_limit: u64,
    #[serde(default)]
    pub description: String,
}

impl HookDefinition {
    pub fn encode_calldata(
        &self,
        metadata: &SettlementMetadata,
        runtime: &RuntimeContext,
    ) -> HookResult<Bytes> {
        let function_sig = &self.function_signature;
        let (function_name, input_types) = Self::parse_function_signature(function_sig)?;

        if self.parameters.len() != input_types.len() {
            return Err(HookError::ParameterCountMismatch {
                function: function_sig.clone(),
                expected: input_types.len(),
                actual: self.parameters.len(),
            });
        }

        let mut values = Vec::new();
        for (i, param) in self.parameters.iter().enumerate() {
            let sol_type = DynSolType::parse(&param.sol_type).map_err(|e| {
                HookError::InvalidSolidityType(param.sol_type.clone(), e.to_string())
            })?;

            if param.sol_type != input_types[i] {
                return Err(HookError::TypeMismatch {
                    param: format!("parameter {}", i),
                    expected: input_types[i].clone(),
                    actual: param.sol_type.clone(),
                });
            }

            let value =
                self.resolve_parameter_value(&param.source, &sol_type, metadata, runtime)?;
            values.push(value);
        }

        let encoded = Self::encode_with_selector(function_name, &values)?;
        Ok(Bytes::from(encoded))
    }

    fn parse_function_signature(sig: &str) -> HookResult<(String, Vec<String>)> {
        let parts: Vec<&str> = sig.splitn(2, '(').collect();
        if parts.len() != 2 {
            return Err(HookError::InvalidFunctionSignature(
                sig.to_string(),
                "Missing '(' in signature".to_string(),
            ));
        }

        let function_name = parts[0].to_string();
        let params_str = parts[1].trim_end_matches(')');

        let input_types = if params_str.is_empty() {
            Vec::new()
        } else {
            params_str
                .split(',')
                .map(|s| s.trim().to_string())
                .collect()
        };

        Ok((function_name, input_types))
    }

    fn encode_with_selector(function_name: String, values: &[DynSolValue]) -> HookResult<Vec<u8>> {
        let type_strs: Vec<String> = values
            .iter()
            .map(|v| match v {
                DynSolValue::Address(_) => "address".to_string(),
                DynSolValue::Uint(_, bits) => format!("uint{}", bits),
                DynSolValue::Int(_, bits) => format!("int{}", bits),
                DynSolValue::Bool(_) => "bool".to_string(),
                DynSolValue::FixedBytes(_, size) => format!("bytes{}", size),
                DynSolValue::Bytes(_) => "bytes".to_string(),
                DynSolValue::String(_) => "string".to_string(),
                _ => "unknown".to_string(),
            })
            .collect();

        let full_signature = format!("{}({})", function_name, type_strs.join(","));
        let selector = alloy::primitives::keccak256(full_signature.as_bytes());
        let selector_bytes = &selector[0..4];

        let tuple_value = DynSolValue::Tuple(values.to_vec());
        let encoded_params = tuple_value.abi_encode_params();

        let mut result = Vec::with_capacity(4 + encoded_params.len());
        result.extend_from_slice(selector_bytes);
        result.extend_from_slice(&encoded_params);

        Ok(result)
    }

    fn resolve_parameter_value(
        &self,
        source: &ParameterSource,
        sol_type: &DynSolType,
        metadata: &SettlementMetadata,
        runtime: &RuntimeContext,
    ) -> HookResult<DynSolValue> {
        match source {
            ParameterSource::Payment(field) => {
                Self::extract_payment_field(field, metadata, sol_type)
            }
            ParameterSource::Runtime(field) => {
                Self::extract_runtime_field(field, runtime, sol_type)
            }
            ParameterSource::Static(val) => Self::parse_static_value(val, sol_type),
            ParameterSource::Config(key) => {
                let val = self.config_values.get(key).ok_or_else(|| {
                    HookError::InvalidParameterSource(
                        key.clone(),
                        "Config key not found in config_values".to_string(),
                    )
                })?;
                Self::parse_static_value(val, sol_type)
            }
        }
    }

    fn extract_payment_field(
        field: &PaymentField,
        metadata: &SettlementMetadata,
        _sol_type: &DynSolType,
    ) -> HookResult<DynSolValue> {
        match field {
            PaymentField::From => Ok(DynSolValue::Address(metadata.from)),
            PaymentField::To => Ok(DynSolValue::Address(metadata.to)),
            PaymentField::Value => Ok(DynSolValue::Uint(metadata.value, 256)),
            PaymentField::ValidAfter => Ok(DynSolValue::Uint(metadata.valid_after, 256)),
            PaymentField::ValidBefore => Ok(DynSolValue::Uint(metadata.valid_before, 256)),
            PaymentField::Nonce => Ok(DynSolValue::FixedBytes(metadata.nonce, 32)),
            PaymentField::ContractAddress => Ok(DynSolValue::Address(metadata.contract_address)),
            PaymentField::SignatureV => {
                let v = if let Some(sig_bytes) = metadata.signature.get(64) {
                    U256::from(*sig_bytes)
                } else {
                    U256::ZERO
                };
                Ok(DynSolValue::Uint(v, 8))
            }
            PaymentField::SignatureR => {
                let r = if metadata.signature.len() >= 32 {
                    FixedBytes::from_slice(&metadata.signature[0..32])
                } else {
                    FixedBytes::ZERO
                };
                Ok(DynSolValue::FixedBytes(r, 32))
            }
            PaymentField::SignatureS => {
                let s = if metadata.signature.len() >= 64 {
                    FixedBytes::from_slice(&metadata.signature[32..64])
                } else {
                    FixedBytes::ZERO
                };
                Ok(DynSolValue::FixedBytes(s, 32))
            }
        }
    }

    fn extract_runtime_field(
        field: &RuntimeField,
        runtime: &RuntimeContext,
        _sol_type: &DynSolType,
    ) -> HookResult<DynSolValue> {
        match field {
            RuntimeField::Timestamp => Ok(DynSolValue::Uint(runtime.timestamp, 256)),
            RuntimeField::BlockNumber => Ok(DynSolValue::Uint(runtime.block_number, 256)),
            RuntimeField::Sender => Ok(DynSolValue::Address(runtime.sender)),
            RuntimeField::BatchIndex => {
                let idx = runtime.batch_index.unwrap_or(0);
                Ok(DynSolValue::Uint(U256::from(idx), 256))
            }
            RuntimeField::BatchSize => {
                let size = runtime.batch_size.unwrap_or(0);
                Ok(DynSolValue::Uint(U256::from(size), 256))
            }
        }
    }

    fn parse_static_value(val: &str, sol_type: &DynSolType) -> HookResult<DynSolValue> {
        match sol_type {
            DynSolType::Address => {
                let addr = Address::from_str(val).map_err(|e| {
                    HookError::StaticValueParseFailed(
                        val.to_string(),
                        "address".to_string(),
                        e.to_string(),
                    )
                })?;
                Ok(DynSolValue::Address(addr))
            }
            DynSolType::Uint(bits) => {
                let uint = if val.starts_with("0x") {
                    U256::from_str_radix(val.trim_start_matches("0x"), 16)
                } else {
                    U256::from_str_radix(val, 10)
                }
                .map_err(|e| {
                    HookError::StaticValueParseFailed(
                        val.to_string(),
                        format!("uint{}", bits),
                        e.to_string(),
                    )
                })?;
                Ok(DynSolValue::Uint(uint, *bits))
            }
            DynSolType::Int(bits) => {
                let abs_val = if val.starts_with("0x") {
                    U256::from_str_radix(val.trim_start_matches("0x"), 16)
                } else if val.starts_with("-") {
                    U256::from_str_radix(val.trim_start_matches("-"), 10)
                } else {
                    U256::from_str_radix(val, 10)
                }
                .map_err(|e| {
                    HookError::StaticValueParseFailed(
                        val.to_string(),
                        format!("int{}", bits),
                        e.to_string(),
                    )
                })?;

                let is_negative = val.starts_with("-");
                let int = if is_negative {
                    alloy::primitives::I256::unchecked_from(abs_val).wrapping_neg()
                } else {
                    alloy::primitives::I256::unchecked_from(abs_val)
                };

                Ok(DynSolValue::Int(int, *bits))
            }
            DynSolType::Bool => {
                let b = val.parse::<bool>().map_err(|e| {
                    HookError::StaticValueParseFailed(
                        val.to_string(),
                        "bool".to_string(),
                        e.to_string(),
                    )
                })?;
                Ok(DynSolValue::Bool(b))
            }
            DynSolType::FixedBytes(size) => {
                let hex_str = val.strip_prefix("0x").unwrap_or(val);
                let bytes = alloy::hex::decode(hex_str).map_err(|e| {
                    HookError::StaticValueParseFailed(
                        val.to_string(),
                        format!("bytes{}", size),
                        e.to_string(),
                    )
                })?;
                if bytes.len() != *size {
                    return Err(HookError::StaticValueParseFailed(
                        val.to_string(),
                        format!("bytes{}", size),
                        format!("Expected {} bytes, got {}", size, bytes.len()),
                    ));
                }
                Ok(DynSolValue::FixedBytes(
                    FixedBytes::from_slice(&bytes),
                    *size,
                ))
            }
            DynSolType::Bytes => {
                let hex_str = val.strip_prefix("0x").unwrap_or(val);
                let bytes = alloy::hex::decode(hex_str).map_err(|e| {
                    HookError::StaticValueParseFailed(
                        val.to_string(),
                        "bytes".to_string(),
                        e.to_string(),
                    )
                })?;
                Ok(DynSolValue::Bytes(bytes.into()))
            }
            DynSolType::String => Ok(DynSolValue::String(val.to_string())),
            _ => Err(HookError::InvalidSolidityType(
                format!("{:?}", sol_type),
                "Unsupported type for static value parsing".to_string(),
            )),
        }
    }

    pub fn validate(&self) -> HookResult<()> {
        let (_, input_types) = Self::parse_function_signature(&self.function_signature)?;

        if self.parameters.len() != input_types.len() {
            return Err(HookError::ParameterCountMismatch {
                function: self.function_signature.clone(),
                expected: input_types.len(),
                actual: self.parameters.len(),
            });
        }

        for (i, param) in self.parameters.iter().enumerate() {
            DynSolType::parse(&param.sol_type).map_err(|e| {
                HookError::InvalidSolidityType(param.sol_type.clone(), e.to_string())
            })?;

            if param.sol_type != input_types[i] {
                return Err(HookError::TypeMismatch {
                    param: format!("parameter {}", i),
                    expected: input_types[i].clone(),
                    actual: param.sol_type.clone(),
                });
            }
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum TokenFilter {
    Any(String),
    Specific(Vec<String>),
}

impl TokenFilter {
    pub fn matches(&self, token_name: &str) -> bool {
        match self {
            TokenFilter::Any(s) if s == "*" => true,
            TokenFilter::Specific(tokens) => tokens.contains(&token_name.to_string()),
            _ => false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkHookConfig {
    pub enabled: Option<bool>,
    #[serde(default)]
    pub mappings: HashMap<String, Vec<String>>,
    #[serde(default)]
    pub contracts: HashMap<String, HashMap<String, String>>,
    #[serde(default)]
    pub token_filters: HashMap<String, TokenFilter>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookConfig {
    #[serde(default)]
    pub hooks: HookSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookSettings {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub allow_hook_failure: bool,
    #[serde(default)]
    pub custom_hooks_file: Option<String>,
    #[serde(default)]
    pub definitions: HashMap<String, HookDefinition>,
    #[serde(default)]
    pub networks: HashMap<String, NetworkHookConfig>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub mappings: HashMap<String, Vec<String>>,
}

impl Default for HookSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            allow_hook_failure: false,
            custom_hooks_file: None,
            definitions: HashMap::new(),
            networks: HashMap::new(),
            mappings: HashMap::new(),
        }
    }
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomHookConfig {
    #[serde(default)]
    pub hooks: CustomHookSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CustomHookSettings {
    #[serde(default)]
    pub definitions: HashMap<String, HookDefinition>,
    #[serde(default)]
    pub networks: HashMap<String, NetworkHookConfig>,
}

impl HookSettings {
    pub fn is_enabled_for_network(&self, network_name: &str) -> bool {
        self.networks
            .get(network_name)
            .and_then(|n| n.enabled)
            .unwrap_or(self.enabled)
    }

    pub fn resolve_contract_address(
        &self,
        hook_name: &str,
        network_name: &str,
        destination: &Address,
    ) -> Option<Address> {
        let network_config = self.networks.get(network_name)?;
        let dest_map = network_config.contracts.get(hook_name)?;

        let dest_str = format!("{:?}", destination);
        let dest_lower = dest_str.to_lowercase();

        for (key, addr) in dest_map {
            if key.to_lowercase() == dest_lower {
                let resolved = Self::substitute_env_var(addr);
                return Address::from_str(&resolved).ok();
            }
        }

        None
    }

    pub fn resolve_mapping_address(address_str: &str) -> Option<Address> {
        let resolved = Self::substitute_env_var(address_str);
        Address::from_str(&resolved).ok()
    }

    fn substitute_env_var(s: &str) -> String {
        if s.starts_with("${") && s.ends_with("}") {
            let env_var_name = &s[2..s.len() - 1];
            std::env::var(env_var_name).unwrap_or_else(|_| {
                tracing::warn!(
                    env_var = env_var_name,
                    "Environment variable not found, using empty address"
                );
                String::new()
            })
        } else {
            s.to_string()
        }
    }

    pub fn get_network_mappings(&self, network_name: &str) -> &HashMap<String, Vec<String>> {
        self.networks
            .get(network_name)
            .map(|n| &n.mappings)
            .filter(|m| !m.is_empty())
            .unwrap_or(&self.mappings)
    }
}

impl HookConfig {
    pub fn from_file(path: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let content = std::fs::read_to_string(path)?;
        let mut config: HookConfig = toml::from_str(&content)?;

        if let Some(ref custom_file) = config.hooks.custom_hooks_file {
            let custom_path = Self::resolve_path(path, custom_file);

            match Self::load_custom_hooks(&custom_path) {
                Ok(custom_config) => {
                    Self::merge_custom_hooks(&mut config.hooks, custom_config)?;
                    tracing::info!(custom_file = custom_path, "Loaded and merged custom hooks");
                }
                Err(e) => {
                    tracing::warn!(
                        custom_file = custom_path,
                        error = %e,
                        "Failed to load custom hooks file (continuing without custom hooks)"
                    );
                }
            }
        }

        for (name, hook) in &config.hooks.definitions {
            hook.validate()
                .map_err(|e| format!("Hook '{}' validation failed: {}", name, e))?;
        }

        tracing::info!(
            path = path,
            definitions_count = config.hooks.definitions.len(),
            networks_count = config.hooks.networks.len(),
            "Loaded hook configuration"
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

    fn load_custom_hooks(path: &str) -> Result<CustomHookConfig, Box<dyn std::error::Error>> {
        let content = std::fs::read_to_string(path)?;
        let config: CustomHookConfig = toml::from_str(&content)?;
        Ok(config)
    }

    fn merge_custom_hooks(
        main: &mut HookSettings,
        custom: CustomHookConfig,
    ) -> Result<(), Box<dyn std::error::Error>> {
        for (name, def) in custom.hooks.definitions {
            if main.definitions.contains_key(&name) {
                tracing::warn!(hook = name, "Custom hook definition overriding production definition");
            }
            main.definitions.insert(name, def);
        }

        for (network_name, custom_network) in custom.hooks.networks {
            let network_config = main
                .networks
                .entry(network_name.clone())
                .or_insert_with(|| NetworkHookConfig {
                    enabled: None,
                    mappings: HashMap::new(),
                    contracts: HashMap::new(),
                    token_filters: HashMap::new(),
                });

            for (dest_addr, hook_names) in custom_network.mappings {
                network_config.mappings.insert(dest_addr, hook_names);
            }

            for (hook_name, dest_contracts) in custom_network.contracts {
                let hook_contracts = network_config
                    .contracts
                    .entry(hook_name)
                    .or_default();
                for (dest_addr, contract_addr) in dest_contracts {
                    hook_contracts.insert(dest_addr, contract_addr);
                }
            }

            for (hook_name, token_filter) in custom_network.token_filters {
                network_config.token_filters.insert(hook_name, token_filter);
            }

            if let Some(custom_enabled) = custom_network.enabled {
                network_config.enabled = Some(custom_enabled);
            }
        }

        Ok(())
    }

    #[cfg(test)]
    pub fn to_file(&self, path: &str) -> Result<(), Box<dyn std::error::Error>> {
        let content = toml::to_string_pretty(self)?;
        std::fs::write(path, content)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parameterized_hook_config_parsing() {
        let toml_str = r#"
[hooks]
enabled = true

[hooks.mappings]
"${NOTIFY_RECIPIENT}" = ["notify_hook"]

[hooks.definitions.notify_hook]
enabled = true
contract = "0x1234567890123456789012345678901234567890"
function_signature = "notifySettlement(address,address,uint256)"
gas_limit = 200000
description = "Notify settlement with dynamic params"

[[hooks.definitions.notify_hook.parameters]]
type = "address"
source = { source_type = "payment", field = "from" }

[[hooks.definitions.notify_hook.parameters]]
type = "address"
source = { source_type = "payment", field = "to" }

[[hooks.definitions.notify_hook.parameters]]
type = "uint256"
source = { source_type = "payment", field = "value" }
"#;

        let config: HookConfig = toml::from_str(toml_str).unwrap();
        assert!(config.hooks.enabled);

        let hook = config.hooks.definitions.get("notify_hook").unwrap();
        assert_eq!(
            &hook.function_signature,
            "notifySettlement(address,address,uint256)"
        );
        assert_eq!(hook.parameters.len(), 3);
    }

    #[test]
    fn test_parse_function_signature() {
        let (name, types) =
            HookDefinition::parse_function_signature("notifySettlement(address,address,uint256)")
                .unwrap();

        assert_eq!(name, "notifySettlement");
        assert_eq!(types, vec!["address", "address", "uint256"]);
    }

    #[test]
    fn test_parse_function_signature_no_params() {
        let (name, types) = HookDefinition::parse_function_signature("trigger()").unwrap();

        assert_eq!(name, "trigger");
        assert!(types.is_empty());
    }
}
