use thiserror::Error;

#[derive(Error, Debug)]
pub enum HookError {
    #[error("Invalid function signature '{0}': {1}")]
    InvalidFunctionSignature(String, String),

    #[error("Invalid Solidity type '{0}': {1}")]
    InvalidSolidityType(String, String),

    #[error("Missing required parameter '{0}' for function '{1}'")]
    MissingParameter(String, String),

    #[error("Invalid parameter source '{0}': {1}")]
    InvalidParameterSource(String, String),

    #[error("ABI encoding failed: {0}")]
    EncodingFailed(String),

    #[error("Failed to parse static value '{0}' as {1}: {2}")]
    StaticValueParseFailed(String, String, String),

    #[error(
        "Parameter count mismatch: expected {expected}, got {actual} for function '{function}'"
    )]
    ParameterCountMismatch {
        function: String,
        expected: usize,
        actual: usize,
    },

    #[error("Type mismatch for parameter '{param}': expected {expected}, got {actual}")]
    TypeMismatch {
        param: String,
        expected: String,
        actual: String,
    },

    #[error("Configuration error: {0}")]
    ConfigError(String),

    #[error("Failed to fetch runtime context: {0}")]
    RuntimeContextFailed(String),

    #[error("Hook definition '{0}' not found")]
    HookNotFound(String),

    #[error("Invalid hex string: {0}")]
    InvalidHex(String),

    #[error("Deprecated: {0}. {1}")]
    DeprecatedFeature(String, String),
}

pub type HookResult<T> = Result<T, HookError>;

impl From<alloy::dyn_abi::Error> for HookError {
    fn from(err: alloy::dyn_abi::Error) -> Self {
        HookError::EncodingFailed(err.to_string())
    }
}

impl From<alloy::hex::FromHexError> for HookError {
    fn from(err: alloy::hex::FromHexError) -> Self {
        HookError::InvalidHex(err.to_string())
    }
}
