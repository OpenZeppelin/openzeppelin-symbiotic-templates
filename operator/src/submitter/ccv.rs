//! OffRamp calldata encoding for Chainlink CCV message execution.

use alloy::primitives::{Address, Bytes};
use alloy::sol;
use alloy::sol_types::SolCall;

sol! {
    #[derive(Debug)]
    interface IOffRampExecute {
        function execute(
            bytes calldata encodedMessage,
            address[] calldata ccvs,
            bytes[] calldata verifierResults,
            uint32 gasLimitOverride
        ) external;
    }
}

/// Encode OffRamp.execute calldata.
pub fn encode_offramp_execute(
    encoded_message: &[u8],
    ccvs: Vec<Address>,
    verifier_results: Vec<Vec<u8>>,
    gas_limit_override: u32,
) -> Bytes {
    let verifier_results: Vec<Bytes> = verifier_results
        .into_iter()
        .map(Bytes::from)
        .collect();

    let call = IOffRampExecute::executeCall {
        encodedMessage: Bytes::copy_from_slice(encoded_message),
        ccvs,
        verifierResults: verifier_results,
        gasLimitOverride: gas_limit_override,
    };

    Bytes::from(call.abi_encode())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_offramp_execute() {
        let calldata = encode_offramp_execute(
            &[0x01, 0x02, 0x03],
            vec!["0x1111111111111111111111111111111111111111".parse().unwrap()],
            vec![vec![0xaa, 0xbb]],
            0,
        );

        assert!(!calldata.is_empty());
        assert!(calldata.len() > 4);
    }
}

