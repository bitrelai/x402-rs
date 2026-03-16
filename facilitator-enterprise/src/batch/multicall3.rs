use alloy::sol;

sol! {
    /// Multicall3 interface for batching multiple calls into a single transaction.
    #[sol(rpc)]
    interface IMulticall3 {
        struct Call3 {
            address target;
            bool allowFailure;
            bytes callData;
        }
        struct Result {
            bool success;
            bytes returnData;
        }
        function aggregate3(Call3[] calldata calls)
            external payable returns (Result[] memory returnData);
    }
}

// EIP-3009 transferWithAuthorization function ABI.
sol! {
    function transferWithAuthorization(
        address from,
        address to,
        uint256 value,
        uint256 validAfter,
        uint256 validBefore,
        bytes32 nonce,
        bytes calldata signature
    ) external;
}

/// Well-known Multicall3 contract address (same on all EVM chains).
pub const MULTICALL3_ADDRESS: alloy::primitives::Address =
    alloy::primitives::address!("cA11bde05977b3631167028862bE2a173976CA11");
