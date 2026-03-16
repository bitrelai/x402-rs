use ipnetwork::IpNetwork;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct EnterpriseConfig {
    pub rate_limiting: RateLimitingConfig,
    pub cors: CorsConfig,
    pub ip_filtering: IpFilteringConfig,
    pub request: RequestConfig,
    pub security: SecurityConfig,
    pub transaction: TransactionConfig,
    pub batch_settlement: BatchSettlementConfig,
}

impl Default for EnterpriseConfig {
    fn default() -> Self {
        Self {
            rate_limiting: RateLimitingConfig::default(),
            cors: CorsConfig::default(),
            ip_filtering: IpFilteringConfig::default(),
            request: RequestConfig::default(),
            security: SecurityConfig::default(),
            transaction: TransactionConfig::default(),
            batch_settlement: BatchSettlementConfig::default(),
        }
    }
}

impl EnterpriseConfig {
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self, config::ConfigError> {
        let path = path.as_ref();

        if !path.exists() {
            tracing::info!("Config file not found at {:?}, using defaults", path);
            return Ok(Self::default());
        }

        config::Config::builder()
            .add_source(config::File::from(path))
            .build()?
            .try_deserialize()
    }

    pub fn from_env() -> Result<Self, config::ConfigError> {
        let config_path =
            std::env::var("CONFIG_FILE").unwrap_or_else(|_| "config.toml".to_string());
        Self::from_file(config_path)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct RateLimitingConfig {
    pub enabled: bool,
    pub requests_per_second: u32,
    pub ban_duration_seconds: u64,
    pub ban_threshold: u32,
    #[serde(default)]
    pub endpoints: HashMap<String, u32>,
    #[serde(with = "ip_list_serde")]
    pub whitelisted_ips: Vec<IpNetwork>,
}

impl Default for RateLimitingConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            requests_per_second: 10,
            ban_duration_seconds: 300,
            ban_threshold: 5,
            endpoints: HashMap::new(),
            whitelisted_ips: default_local_whitelist(),
        }
    }
}

fn default_local_whitelist() -> Vec<IpNetwork> {
    use std::str::FromStr;
    vec![
        IpNetwork::from_str("127.0.0.0/8").unwrap(),
        IpNetwork::from_str("::1/128").unwrap(),
        IpNetwork::from_str("::ffff:127.0.0.0/104").unwrap(),
    ]
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct CorsConfig {
    pub allowed_origins: Vec<String>,
}

impl Default for CorsConfig {
    fn default() -> Self {
        Self {
            allowed_origins: vec![],
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct IpFilteringConfig {
    #[serde(with = "ip_list_serde")]
    pub allowed_ips: Vec<IpNetwork>,
    #[serde(with = "ip_list_serde")]
    pub blocked_ips: Vec<IpNetwork>,
}

impl Default for IpFilteringConfig {
    fn default() -> Self {
        Self {
            allowed_ips: vec![],
            blocked_ips: vec![],
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct RequestConfig {
    pub max_body_size_bytes: usize,
}

impl Default for RequestConfig {
    fn default() -> Self {
        Self {
            max_body_size_bytes: 1_048_576,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct SecurityConfig {
    pub health_endpoint_requires_auth: bool,
    pub log_security_events: bool,
    pub cleanup_interval_seconds: u64,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            health_endpoint_requires_auth: false,
            log_security_events: true,
            cleanup_interval_seconds: 300,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ChainConfig {
    pub block_time_seconds: u64,
    pub receipt_timeout_blocks: u64,
    pub rpc_request_timeout_seconds: u64,
    #[serde(default)]
    pub gas_buffer: Option<f64>,
}

impl ChainConfig {
    #[allow(dead_code)] // Used when per-chain transaction config is wired
    pub fn receipt_timeout(&self) -> Duration {
        Duration::from_secs(self.block_time_seconds * self.receipt_timeout_blocks)
    }

    #[allow(dead_code)]
    pub fn rpc_timeout(&self) -> Duration {
        Duration::from_secs(self.rpc_request_timeout_seconds)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct TransactionConfig {
    pub default_rpc_timeout_seconds: u64,
    pub connection_timeout_seconds: u64,
    pub pool_max_idle_per_host: usize,
    pub pool_idle_timeout_seconds: u64,
    #[serde(default)]
    pub chains: HashMap<String, ChainConfig>,
    #[serde(default = "default_gas_buffer")]
    pub gas_buffer: f64,
}

fn default_gas_buffer() -> f64 {
    1.0
}

impl Default for TransactionConfig {
    fn default() -> Self {
        Self {
            default_rpc_timeout_seconds: 30,
            connection_timeout_seconds: 10,
            pool_max_idle_per_host: 100,
            pool_idle_timeout_seconds: 90,
            chains: HashMap::new(),
            gas_buffer: default_gas_buffer(),
        }
    }
}

impl TransactionConfig {
    #[allow(dead_code)]
    pub fn gas_buffer_for_network(&self, network: &str) -> f64 {
        self.chains
            .get(network)
            .and_then(|c| c.gas_buffer)
            .unwrap_or(self.gas_buffer)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct BatchSettlementConfig {
    pub enabled: bool,
    pub max_batch_size: usize,
    pub max_wait_ms: u64,
    pub min_batch_size: usize,
    pub allow_partial_failure: bool,
    pub allow_hook_failure: bool,
    #[serde(default)]
    pub networks: HashMap<String, NetworkBatchConfig>,
}

impl Default for BatchSettlementConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_batch_size: 150,
            max_wait_ms: 500,
            min_batch_size: 10,
            allow_partial_failure: false,
            allow_hook_failure: false,
            networks: HashMap::new(),
        }
    }
}

impl BatchSettlementConfig {
    pub fn is_enabled_for_network(&self, network_name: &str) -> bool {
        self.networks
            .get(network_name)
            .and_then(|n| n.enabled)
            .unwrap_or(self.enabled)
    }

    pub fn is_enabled_anywhere(&self) -> bool {
        self.enabled || self.networks.values().any(|n| n.enabled == Some(true))
    }

    pub fn for_network(&self, network_name: &str) -> ResolvedBatchConfig {
        let network_override = self.networks.get(network_name);

        ResolvedBatchConfig {
            max_batch_size: network_override
                .and_then(|n| n.max_batch_size)
                .unwrap_or(self.max_batch_size),
            max_wait_ms: network_override
                .and_then(|n| n.max_wait_ms)
                .unwrap_or(self.max_wait_ms),
            min_batch_size: network_override
                .and_then(|n| n.min_batch_size)
                .unwrap_or(self.min_batch_size),
            allow_partial_failure: network_override
                .and_then(|n| n.allow_partial_failure)
                .unwrap_or(self.allow_partial_failure),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct NetworkBatchConfig {
    pub enabled: Option<bool>,
    pub max_batch_size: Option<usize>,
    pub max_wait_ms: Option<u64>,
    pub min_batch_size: Option<usize>,
    pub allow_partial_failure: Option<bool>,
}

#[derive(Debug, Clone, Copy)]
pub struct ResolvedBatchConfig {
    pub max_batch_size: usize,
    pub max_wait_ms: u64,
    #[allow(dead_code)] // Read by infra402 queue's min-flush logic
    pub min_batch_size: usize,
    pub allow_partial_failure: bool,
}

mod ip_list_serde {
    use ipnetwork::IpNetwork;
    use serde::{Deserialize, Deserializer, Serializer};
    use std::str::FromStr;

    pub fn serialize<S>(ips: &Vec<IpNetwork>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let strings: Vec<String> = ips.iter().map(|ip| ip.to_string()).collect();
        serializer.collect_seq(strings)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<IpNetwork>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let strings: Vec<String> = Vec::deserialize(deserializer)?;
        strings
            .into_iter()
            .map(|s| IpNetwork::from_str(&s).map_err(serde::de::Error::custom))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = EnterpriseConfig::default();
        assert!(config.rate_limiting.enabled);
        assert_eq!(config.rate_limiting.requests_per_second, 10);
        assert_eq!(config.request.max_body_size_bytes, 1_048_576);
    }

    #[test]
    fn test_parse_ip_networks() {
        let config_str = r#"
[ip_filtering]
allowed_ips = ["192.168.1.0/24", "10.0.0.1"]
blocked_ips = ["192.0.2.0/24"]
"#;

        let config: EnterpriseConfig = toml::from_str(config_str).unwrap();
        assert_eq!(config.ip_filtering.allowed_ips.len(), 2);
        assert_eq!(config.ip_filtering.blocked_ips.len(), 1);
    }

    #[test]
    fn test_batch_settlement_default() {
        let config = BatchSettlementConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.max_batch_size, 150);
        assert_eq!(config.max_wait_ms, 500);
        assert_eq!(config.min_batch_size, 10);
        assert!(!config.allow_partial_failure);
        assert!(config.networks.is_empty());
    }

    #[test]
    fn test_batch_settlement_per_network_config() {
        let config_str = r#"
[batch_settlement]
enabled = true
max_batch_size = 150
max_wait_ms = 500
min_batch_size = 10
allow_partial_failure = false

[batch_settlement.networks.bsc]
max_batch_size = 200
allow_partial_failure = true

[batch_settlement.networks.base]
max_wait_ms = 250
min_batch_size = 5
"#;

        let config: EnterpriseConfig = toml::from_str(config_str).unwrap();

        let global = config.batch_settlement.for_network("unknown-network");
        assert_eq!(global.max_batch_size, 150);
        assert_eq!(global.max_wait_ms, 500);

        let bsc = config.batch_settlement.for_network("bsc");
        assert_eq!(bsc.max_batch_size, 200);
        assert!(bsc.allow_partial_failure);

        let base = config.batch_settlement.for_network("base");
        assert_eq!(base.max_wait_ms, 250);
        assert_eq!(base.min_batch_size, 5);
    }

    #[test]
    fn test_batch_settlement_per_network_enabled_override() {
        let config_str = r#"
[batch_settlement]
enabled = false

[batch_settlement.networks.bsc-testnet]
enabled = true
max_batch_size = 100
"#;

        let config: EnterpriseConfig = toml::from_str(config_str).unwrap();

        assert!(!config.batch_settlement.enabled);
        assert!(
            config
                .batch_settlement
                .is_enabled_for_network("bsc-testnet")
        );
        assert!(!config.batch_settlement.is_enabled_for_network("base"));
        assert!(config.batch_settlement.is_enabled_anywhere());
    }
}
