use std::future::Future;
use std::str::FromStr;

use alloy::primitives::{Address, Bytes, U256};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::signers::local::PrivateKeySigner;
use alloy::sol;
use eyre::Result;

#[cfg(test)]
use crate::runner::{CommandRunner, CommandSpec, FakeRunner};
#[cfg(test)]
use eyre::eyre;

sol! {
    #[sol(rpc)]
    interface SettlementAware {
        function settlement() external view returns (address);
    }

    #[sol(rpc)]
    interface SettlementReader {
        function getLastCommittedHeaderEpoch() external view returns (uint48);
        function getCaptureTimestampFromValSetHeaderAt(uint48 epoch) external view returns (uint48);
    }

    #[sol(rpc)]
    interface DriverReader {
        function getCurrentEpoch() external view returns (uint48);
        function getEpochStart(uint48 epoch) external view returns (uint48);
    }

    #[sol(rpc)]
    interface VerifierReader {
        function maxEpochValidity() external view returns (uint256);
    }

    #[sol(rpc)]
    interface KeyRegistryReader {
        function getKey(address operator, uint8 tag) external view returns (bytes);
    }
}

pub trait EthApi {
    fn rpc_reachable(&self, rpc_url: &str) -> bool;
    fn chain_id(&self, rpc_url: &str) -> Result<u64>;
    fn address_from_private_key(&self, private_key: &str) -> Result<Address>;
    fn balance(&self, rpc_url: &str, address: Address) -> Result<U256>;
    fn has_code(&self, rpc_url: &str, address: Address) -> Result<bool>;
    fn nonce(&self, rpc_url: &str, address: Address) -> Result<u64>;
    fn settlement_address(&self, rpc_url: &str, address: Address) -> Result<Address>;
    /// Reads `maxEpochValidity()` from a deployed SymbioticVerifier. Errors when
    /// the contract predates the epoch-validity remediation (no such function).
    fn max_epoch_validity(&self, rpc_url: &str, verifier: Address) -> Result<u64>;
    fn last_committed_header_epoch(&self, rpc_url: &str, settlement: Address) -> Result<u64>;
    fn capture_timestamp(&self, rpc_url: &str, settlement: Address, epoch: u64) -> Result<u64>;
    fn current_epoch(&self, rpc_url: &str, driver: Address) -> Result<u64>;
    fn epoch_start(&self, rpc_url: &str, driver: Address, epoch: u64) -> Result<u64>;
    fn key_bytes(
        &self,
        rpc_url: &str,
        key_registry: Address,
        operator: Address,
        tag: u8,
    ) -> Result<Vec<u8>>;
    fn block_number(&self, rpc_url: &str) -> Result<u64>;
    fn mine_block(&self, rpc_url: &str) -> Result<()>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct AlloyEth;

impl EthApi for AlloyEth {
    fn rpc_reachable(&self, rpc_url: &str) -> bool {
        self.chain_id(rpc_url).is_ok()
    }

    fn chain_id(&self, rpc_url: &str) -> Result<u64> {
        self.block_on(async move {
            let provider = ProviderBuilder::new().on_http(rpc_url.parse()?);
            Ok(provider.get_chain_id().await?)
        })
    }

    fn address_from_private_key(&self, private_key: &str) -> Result<Address> {
        let signer: PrivateKeySigner = private_key.parse()?;
        Ok(signer.address())
    }

    fn balance(&self, rpc_url: &str, address: Address) -> Result<U256> {
        self.block_on(async move {
            let provider = ProviderBuilder::new().on_http(rpc_url.parse()?);
            Ok(provider.get_balance(address).await?)
        })
    }

    fn has_code(&self, rpc_url: &str, address: Address) -> Result<bool> {
        self.block_on(async move {
            let provider = ProviderBuilder::new().on_http(rpc_url.parse()?);
            Ok(!provider.get_code_at(address).await?.is_empty())
        })
    }

    fn nonce(&self, rpc_url: &str, address: Address) -> Result<u64> {
        self.block_on(async move {
            let provider = ProviderBuilder::new().on_http(rpc_url.parse()?);
            Ok(provider.get_transaction_count(address).await?)
        })
    }

    fn settlement_address(&self, rpc_url: &str, address: Address) -> Result<Address> {
        self.block_on(async move {
            let provider = ProviderBuilder::new().on_http(rpc_url.parse()?);
            let contract = SettlementAware::new(address, provider);
            Ok(contract.settlement().call().await?._0)
        })
    }

    fn max_epoch_validity(&self, rpc_url: &str, verifier: Address) -> Result<u64> {
        self.block_on(async move {
            let provider = ProviderBuilder::new().on_http(rpc_url.parse()?);
            let contract = VerifierReader::new(verifier, provider);
            Ok(contract.maxEpochValidity().call().await?._0.to::<u64>())
        })
    }

    fn last_committed_header_epoch(&self, rpc_url: &str, settlement: Address) -> Result<u64> {
        self.block_on(async move {
            let provider = ProviderBuilder::new().on_http(rpc_url.parse()?);
            let contract = SettlementReader::new(settlement, provider);
            Ok(contract
                .getLastCommittedHeaderEpoch()
                .call()
                .await?
                ._0
                .to::<u64>())
        })
    }

    fn capture_timestamp(&self, rpc_url: &str, settlement: Address, epoch: u64) -> Result<u64> {
        self.block_on(async move {
            let provider = ProviderBuilder::new().on_http(rpc_url.parse()?);
            let contract = SettlementReader::new(settlement, provider);
            let epoch = alloy::primitives::Uint::<48, 1>::from_str(&epoch.to_string())?;
            Ok(contract
                .getCaptureTimestampFromValSetHeaderAt(epoch)
                .call()
                .await?
                ._0
                .to::<u64>())
        })
    }

    fn current_epoch(&self, rpc_url: &str, driver: Address) -> Result<u64> {
        self.block_on(async move {
            let provider = ProviderBuilder::new().on_http(rpc_url.parse()?);
            let contract = DriverReader::new(driver, provider);
            Ok(contract.getCurrentEpoch().call().await?._0.to::<u64>())
        })
    }

    fn epoch_start(&self, rpc_url: &str, driver: Address, epoch: u64) -> Result<u64> {
        self.block_on(async move {
            let provider = ProviderBuilder::new().on_http(rpc_url.parse()?);
            let contract = DriverReader::new(driver, provider);
            let epoch = alloy::primitives::Uint::<48, 1>::from_str(&epoch.to_string())?;
            Ok(contract.getEpochStart(epoch).call().await?._0.to::<u64>())
        })
    }

    fn key_bytes(
        &self,
        rpc_url: &str,
        key_registry: Address,
        operator: Address,
        tag: u8,
    ) -> Result<Vec<u8>> {
        self.block_on(async move {
            let provider = ProviderBuilder::new().on_http(rpc_url.parse()?);
            let contract = KeyRegistryReader::new(key_registry, provider);
            let key: Bytes = contract.getKey(operator, tag).call().await?._0;
            Ok(key.to_vec())
        })
    }

    fn block_number(&self, rpc_url: &str) -> Result<u64> {
        self.block_on(async move {
            let provider = ProviderBuilder::new().on_http(rpc_url.parse()?);
            Ok(provider.get_block_number().await?)
        })
    }

    fn mine_block(&self, rpc_url: &str) -> Result<()> {
        self.block_on(async move {
            let provider = ProviderBuilder::new().on_http(rpc_url.parse()?);
            let _: String = provider.raw_request("evm_mine".into(), ()).await?;
            Ok(())
        })
    }
}

impl AlloyEth {
    fn block_on<T>(&self, future: impl Future<Output = Result<T>>) -> Result<T> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        runtime.block_on(future)
    }
}

pub fn parse_address(value: &str) -> Option<Address> {
    value.parse().ok()
}

#[cfg(test)]
impl EthApi for FakeRunner {
    fn rpc_reachable(&self, rpc_url: &str) -> bool {
        self.run(&CommandSpec::new(
            "cast",
            vec![
                "client".to_string(),
                "--rpc-url".to_string(),
                rpc_url.to_string(),
            ],
        ))
        .map(|output| output.success)
        .unwrap_or(false)
    }

    fn chain_id(&self, rpc_url: &str) -> Result<u64> {
        parse_u64(fake_output(
            self,
            vec![
                "chain-id".to_string(),
                "--rpc-url".to_string(),
                rpc_url.to_string(),
            ],
        )?)
    }

    fn address_from_private_key(&self, private_key: &str) -> Result<Address> {
        parse_address_required(fake_output(
            self,
            vec![
                "wallet".to_string(),
                "address".to_string(),
                "--private-key".to_string(),
                private_key.to_string(),
            ],
        )?)
    }

    fn balance(&self, rpc_url: &str, address: Address) -> Result<U256> {
        parse_u256(fake_output(
            self,
            vec![
                "balance".to_string(),
                address.to_string(),
                "--rpc-url".to_string(),
                rpc_url.to_string(),
            ],
        )?)
    }

    fn has_code(&self, rpc_url: &str, address: Address) -> Result<bool> {
        Ok(fake_output(
            self,
            vec![
                "code".to_string(),
                address.to_string(),
                "--rpc-url".to_string(),
                rpc_url.to_string(),
            ],
        )? != "0x")
    }

    fn nonce(&self, rpc_url: &str, address: Address) -> Result<u64> {
        parse_u64(fake_output(
            self,
            vec![
                "nonce".to_string(),
                address.to_string(),
                "--rpc-url".to_string(),
                rpc_url.to_string(),
            ],
        )?)
    }

    fn settlement_address(&self, rpc_url: &str, address: Address) -> Result<Address> {
        parse_address_required(fake_output(
            self,
            vec![
                "call".to_string(),
                address.to_string(),
                "settlement()(address)".to_string(),
                "--rpc-url".to_string(),
                rpc_url.to_string(),
            ],
        )?)
    }

    fn max_epoch_validity(&self, rpc_url: &str, verifier: Address) -> Result<u64> {
        parse_u64(fake_output(
            self,
            vec![
                "call".to_string(),
                verifier.to_string(),
                "maxEpochValidity()(uint256)".to_string(),
                "--rpc-url".to_string(),
                rpc_url.to_string(),
            ],
        )?)
    }

    fn last_committed_header_epoch(&self, rpc_url: &str, settlement: Address) -> Result<u64> {
        parse_u64(fake_output(
            self,
            vec![
                "call".to_string(),
                settlement.to_string(),
                "getLastCommittedHeaderEpoch()(uint48)".to_string(),
                "--rpc-url".to_string(),
                rpc_url.to_string(),
            ],
        )?)
    }

    fn capture_timestamp(&self, rpc_url: &str, settlement: Address, epoch: u64) -> Result<u64> {
        parse_u64(fake_output(
            self,
            vec![
                "call".to_string(),
                settlement.to_string(),
                "getCaptureTimestampFromValSetHeaderAt(uint48)(uint48)".to_string(),
                epoch.to_string(),
                "--rpc-url".to_string(),
                rpc_url.to_string(),
            ],
        )?)
    }

    fn current_epoch(&self, rpc_url: &str, driver: Address) -> Result<u64> {
        parse_u64(fake_output(
            self,
            vec![
                "call".to_string(),
                driver.to_string(),
                "getCurrentEpoch()(uint48)".to_string(),
                "--rpc-url".to_string(),
                rpc_url.to_string(),
            ],
        )?)
    }

    fn epoch_start(&self, rpc_url: &str, driver: Address, epoch: u64) -> Result<u64> {
        parse_u64(fake_output(
            self,
            vec![
                "call".to_string(),
                driver.to_string(),
                "getEpochStart(uint48)(uint48)".to_string(),
                epoch.to_string(),
                "--rpc-url".to_string(),
                rpc_url.to_string(),
            ],
        )?)
    }

    fn key_bytes(
        &self,
        rpc_url: &str,
        key_registry: Address,
        operator: Address,
        tag: u8,
    ) -> Result<Vec<u8>> {
        parse_bytes(fake_output(
            self,
            vec![
                "call".to_string(),
                key_registry.to_string(),
                "getKey(address,uint8)(bytes)".to_string(),
                operator.to_string(),
                tag.to_string(),
                "--rpc-url".to_string(),
                rpc_url.to_string(),
            ],
        )?)
    }

    fn block_number(&self, rpc_url: &str) -> Result<u64> {
        parse_u64(fake_output(
            self,
            vec![
                "block-number".to_string(),
                "--rpc-url".to_string(),
                rpc_url.to_string(),
            ],
        )?)
    }

    fn mine_block(&self, rpc_url: &str) -> Result<()> {
        fake_output(
            self,
            vec![
                "rpc".to_string(),
                "evm_mine".to_string(),
                "--rpc-url".to_string(),
                rpc_url.to_string(),
            ],
        )?;
        Ok(())
    }
}

#[cfg(test)]
fn fake_output(runner: &FakeRunner, args: Vec<String>) -> Result<String> {
    let output = runner.run(&CommandSpec::new("cast", args))?;
    if !output.success {
        return Err(eyre!("fake cast command failed"));
    }
    let trimmed = output.stdout.trim();
    if trimmed.is_empty() {
        Err(eyre!("fake cast command returned empty output"))
    } else {
        Ok(trimmed.to_string())
    }
}

#[cfg(test)]
fn parse_address_required(value: String) -> Result<Address> {
    parse_address(&value).ok_or_else(|| eyre!("invalid address output: {value}"))
}

#[cfg(test)]
fn parse_u64(value: String) -> Result<u64> {
    value
        .split_whitespace()
        .next()
        .ok_or_else(|| eyre!("missing u64 output"))?
        .parse()
        .map_err(Into::into)
}

#[cfg(test)]
fn parse_u256(value: String) -> Result<U256> {
    value
        .split_whitespace()
        .next()
        .ok_or_else(|| eyre!("missing u256 output"))?
        .parse()
        .map_err(Into::into)
}

#[cfg(test)]
fn parse_bytes(value: String) -> Result<Vec<u8>> {
    let token = value
        .split_whitespace()
        .next()
        .ok_or_else(|| eyre!("missing bytes output"))?;
    if token == "0x" {
        return Ok(Vec::new());
    }
    let token = token.strip_prefix("0x").unwrap_or(token);
    alloy::hex::decode(token).map_err(Into::into)
}
