use std::collections::HashMap;

use alloy::hex::FromHexError;
use alloy::providers::Provider;
use alloy::transports::Transport;
use futures::StreamExt;
use log::{error, info};
use ruint::Uint;
use thiserror::Error;

use config::Config;

use crate::{BridgeResult, contracts};
use crate::source::{EthereumTransaction, RequiredApprovalDetails, RouteSource};
use crate::token_price::TokenPriceProvider;
use crate::token_price::utils::{Errors, get_token_amount_from_value_in_usd};

pub struct SettlementEngine<
    'config,
    'key,
    Source: RouteSource,
    PriceProvider: TokenPriceProvider,
    Erc20Transport: Transport + Clone,
    Erc20Provider: Provider<Erc20Transport>,
> {
    source: Source,
    config: &'config Config,
    price_provider: PriceProvider,
    // (chain_id, token_address) -> Utils
    erc20_instance_map: HashMap<
        (u32, &'key String),
        contracts::ERC20ContractInstance<Erc20Transport, Erc20Provider>,
    >,
}

#[derive(Debug, PartialEq)]
pub enum TransactionType {
    Approval,
    Bungee,
}

#[derive(Debug)]
pub struct TransactionWithType {
    transaction: EthereumTransaction,
    transaction_type: TransactionType,
}

const GENERATE_TRANSACTIONS_CONCURRENCY: usize = 10;

impl<
        'config,
        'key,
        Source: RouteSource,
        PriceProvider: TokenPriceProvider,
        Erc20Transport: Transport + Clone,
        Erc20Provider: Provider<Erc20Transport>,
    > SettlementEngine<'config, 'key, Source, PriceProvider, Erc20Transport, Erc20Provider>
{
    pub fn new(
        config: &'config Config,
        source: Source,
        price_provider: PriceProvider,
        erc20_instance_map: HashMap<
            (u32, &'key String),
            contracts::ERC20ContractInstance<Erc20Transport, Erc20Provider>,
        >,
    ) -> Self {
        SettlementEngine { source, config, price_provider, erc20_instance_map }
    }

    async fn generate_transactions(
        &self,
        routes: &Vec<BridgeResult<'config>>,
    ) -> Result<Vec<TransactionWithType>, SettlementEngineErrors<Source, PriceProvider>> {
        info!("Generating transactions for routes: {:?}", routes);

        let (results, failed): (
            Vec<
                Result<
                    (Vec<EthereumTransaction>, Vec<RequiredApprovalDetails>),
                    SettlementEngineErrors<Source, PriceProvider>,
                >,
            >,
            _,
        ) = futures::stream::iter(routes.into_iter())
            .map(|route| async move {
                info!("Generating transactions for route: {:?}", route.route);

                let token_amount = get_token_amount_from_value_in_usd(
                    self.config,
                    &self.price_provider,
                    &route.route.from_token.symbol,
                    route.route.from_chain.id,
                    &route.source_amount_in_usd,
                )
                .await
                .map_err(|err| SettlementEngineErrors::GetTokenAmountFromValueInUsdError(err))?;

                info!("Token amount: {:?} for route {:?}", token_amount, route);

                let (ethereum_transactions, required_approval_details) = self
                    .source
                    .generate_route_transactions(
                        &route.route,
                        &token_amount,
                        &route.from_address,
                        &route.to_address,
                    )
                    .await
                    .map_err(|err| SettlementEngineErrors::GenerateTransactionsError(err))?;

                info!("Generated transactions: {:?} for route {:?}", ethereum_transactions, route);

                Ok::<_, SettlementEngineErrors<_, _>>((
                    ethereum_transactions,
                    required_approval_details,
                ))
            })
            .buffer_unordered(GENERATE_TRANSACTIONS_CONCURRENCY)
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .partition(Result::is_ok);

        let failed: Vec<_> = failed.into_iter().map(Result::unwrap_err).collect();
        if !failed.is_empty() {
            error!("Failed to generate transactions: {:?}", failed);
        }

        if results.is_empty() {
            error!("No transactions generated");
            return Err(SettlementEngineErrors::NoTransactionsGenerated);
        }

        let (bridge_transactions, required_approval_details): (Vec<Vec<_>>, Vec<Vec<_>>) =
            results.into_iter().map(Result::unwrap).unzip();

        let bridge_transactions: Vec<_> = bridge_transactions
            .into_iter()
            .flatten()
            .map(|t| TransactionWithType {
                transaction: t,
                transaction_type: TransactionType::Bungee,
            })
            .collect();

        let required_approval_details: Vec<_> =
            required_approval_details.into_iter().flatten().collect();
        let required_approval_transactions =
            self.generate_transactions_for_approvals(&required_approval_details).await?;

        info!("Generated transactions: {:?}", bridge_transactions);
        info!("Required approvals: {:?}", required_approval_details);

        let final_transactions = vec![required_approval_transactions, bridge_transactions]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();

        info!("Final Transactions: {:?}", final_transactions);

        Ok(final_transactions)
    }

    async fn generate_transaction_for_approval(
        &self,
        required_approval_details: &RequiredApprovalDetails,
    ) -> Result<Option<TransactionWithType>, SettlementEngineErrors<Source, PriceProvider>> {
        info!("Generating transaction for approval: {:?}", required_approval_details);

        let token_instance = self
            .erc20_instance_map
            .get(&(required_approval_details.chain_id, &required_approval_details.token_address));

        if token_instance.is_none() {
            error!(
                "ERC20 Utils not found for chain_id: {} and token_address: {}",
                required_approval_details.chain_id, required_approval_details.token_address
            );
            return Err(SettlementEngineErrors::ERC20UtilsNotFound(
                required_approval_details.chain_id.clone(),
                required_approval_details.token_address.clone(),
            ));
        }
        let token_instance = token_instance.unwrap();

        let owner = (&required_approval_details.owner)
            .parse()
            .map_err(SettlementEngineErrors::InvalidAddressError)?;
        let spender = (&required_approval_details.target)
            .parse()
            .map_err(SettlementEngineErrors::InvalidAddressError)?;

        let current_approval = token_instance.allowance(owner, spender).call().await?.allowance;

        info!(
            "Current approval: {} on chain against requirement: {:?}",
            current_approval, required_approval_details
        );

        if current_approval >= required_approval_details.amount {
            info!("Sufficient Approval already exists for: {:?}", required_approval_details);
            return Ok(None);
        }

        let required_approval = required_approval_details.amount - current_approval;
        info!(
            "Required Approval: {:?} against requirement: {:?}",
            required_approval, required_approval_details
        );

        let calldata = token_instance.approve(spender, required_approval).calldata().to_string();

        Ok(Some(TransactionWithType {
            transaction: EthereumTransaction {
                from: required_approval_details.owner.clone(),
                to: token_instance.address().to_string(),
                value: Uint::ZERO,
                calldata,
            },
            transaction_type: TransactionType::Approval,
        }))
    }

    async fn generate_transactions_for_approvals(
        &self,
        approvals: &Vec<RequiredApprovalDetails>,
    ) -> Result<Vec<TransactionWithType>, SettlementEngineErrors<Source, PriceProvider>> {
        info!("Generating transactions for approvals: {:?}", approvals);

        // Group the approvals and combine them based on chain_id, token_address, spender and target
        let mut approvals_grouped =
            HashMap::<(u32, &String, &String, &String), Vec<&RequiredApprovalDetails>>::new();
        for approval in approvals {
            let key =
                (approval.chain_id, &approval.token_address, &approval.owner, &approval.target);
            let arr = approvals_grouped.get_mut(&key);
            if arr.is_none() {
                approvals_grouped.insert(key, vec![&approval]);
            } else {
                arr.unwrap().push(&approval);
            }
        }

        // Merge the approvals with the same key
        let merged_approvals: Vec<RequiredApprovalDetails> = approvals_grouped
            .into_iter()
            .map(|(_, approvals)| {
                // If there's only one approval in this group, return it
                if approvals.len() == 1 {
                    return approvals[0].clone();
                }

                let mut amount =
                    approvals.iter().map(|approval| approval.amount).reduce(|a, b| (a + b));

                if amount.is_none() {
                    error!(
                        "Failed to merge approvals due to error in amount reduction: {:?}",
                        approvals
                    );

                    // Set 0 approval if there's an error
                    amount = Some(Uint::ZERO);
                }

                let amount = amount.unwrap();

                RequiredApprovalDetails {
                    chain_id: approvals[0].chain_id,
                    token_address: approvals[0].token_address.clone(),
                    owner: approvals[0].owner.clone(),
                    target: approvals[0].target.clone(),
                    amount,
                }
            })
            .collect();

        // Generate Transactions for the merged approvals
        let (approval_transactions, failed): (Vec<_>, _) =
            futures::stream::iter(merged_approvals.into_iter())
                .map(|approval| async move {
                    Ok::<_, SettlementEngineErrors<_, _>>(
                        self.generate_transaction_for_approval(&approval).await?,
                    )
                })
                .buffer_unordered(GENERATE_TRANSACTIONS_CONCURRENCY)
                .collect::<Vec<_>>()
                .await
                .into_iter()
                .partition(Result::is_ok);

        if !failed.is_empty() {
            error!("Failed to generate approval transactions: {:?}", failed);
        }

        if approval_transactions.is_empty() {
            info!("No Approval Transactions Required");
            return Ok(Vec::new());
        }

        Ok(approval_transactions
            .into_iter()
            .map(Result::unwrap)
            .filter(Option::is_some)
            .map(Option::unwrap)
            .collect())
    }
}

#[derive(Error, Debug)]
pub enum SettlementEngineErrors<Source: RouteSource, PriceProvider: TokenPriceProvider> {
    #[error("Error generating transactions: {0}")]
    GenerateTransactionsError(Source::GenerateRouteTransactionsError),

    #[error("Error getting token amount from value in USD: {0}")]
    GetTokenAmountFromValueInUsdError(Errors<PriceProvider::Error>),

    #[error("No transactions generated")]
    NoTransactionsGenerated,

    #[error("ERC20 Utils not found for chain_id: {0} and token_address: {1}")]
    ERC20UtilsNotFound(u32, String),

    #[error("Error parsing address: {0}")]
    InvalidAddressError(FromHexError),

    #[error("Error while calling ERC20 Contract: {0}")]
    AlloyError(#[from] alloy::contract::Error),
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::env;
    use std::time::Duration;

    use alloy::primitives::U256;
    use alloy::providers::{ProviderBuilder, RootProvider};
    use alloy::transports::http::Http;
    use derive_more::Display;
    use reqwest::{Client, Url};
    use thiserror::Error;

    use config::{Config, get_sample_config};
    use storage::{KeyValueStore, RedisClientError};

    use crate::{BridgeResult, BungeeClient, CoingeckoClient};
    use crate::contracts::ERC20ContractInstance;
    use crate::settlement_engine::{SettlementEngine, TransactionType};
    use crate::source::RequiredApprovalDetails;

    #[derive(Error, Debug, Display)]
    struct Err;

    #[derive(Default, Debug)]
    struct KVStore {
        map: RefCell<HashMap<String, String>>,
    }

    impl KeyValueStore for KVStore {
        type Error = Err;

        async fn get(&self, k: &String) -> Result<String, Self::Error> {
            match self.map.borrow().get(k) {
                Some(v) => Ok(v.clone()),
                None => Result::Err(Err),
            }
        }

        async fn get_multiple(&self, _: &Vec<String>) -> Result<Vec<String>, Self::Error> {
            unimplemented!()
        }

        async fn set(&self, k: &String, v: &String, _: Duration) -> Result<(), Self::Error> {
            self.map
                .borrow_mut()
                .insert((*k.clone()).parse().unwrap(), (*v.clone()).parse().unwrap());
            Ok(())
        }

        async fn set_multiple(&self, _: &Vec<(String, String)>) -> Result<(), Self::Error> {
            unimplemented!()
        }

        async fn get_all_keys(&self) -> Result<Vec<String>, RedisClientError> {
            unimplemented!()
        }

        async fn get_all_key_values(&self) -> Result<HashMap<String, String>, RedisClientError> {
            unimplemented!()
        }
    }

    fn setup_config<'a>() -> Config {
        get_sample_config()
    }

    fn setup<'config: 'key, 'key>(
        config: &'config Config,
    ) -> SettlementEngine<
        'config,
        'key,
        BungeeClient,
        CoingeckoClient<KVStore>,
        Http<Client>,
        RootProvider<Http<Client>>,
    > {
        let bungee_client = BungeeClient::new(
            &"https://api.socket.tech/v2".to_string(),
            &env::var("BUNGEE_API_KEY").unwrap().to_string(),
        )
        .unwrap();

        let client = CoingeckoClient::new(
            config.coingecko.base_url.clone(),
            env::var("COINGECKO_API_KEY").unwrap(),
            KVStore::default(),
            Duration::from_secs(config.coingecko.expiry_sec),
        );

        let erc20_instance_map = config
            .tokens
            .iter()
            .flat_map(|(_, token)| {
                token
                    .by_chain
                    .iter()
                    .map(|(chain_id, chain_specific_config)| {
                        let rpc_url = &config
                            .chains
                            .iter()
                            .find(|(&id, _)| id == *chain_id)
                            .expect(format!("Chain not found for id: {}", chain_id).as_str())
                            .1
                            .rpc_url;

                        let provider = ProviderBuilder::new().on_http(
                            Url::parse(rpc_url.as_str())
                                .expect(format!("Invalid RPC URL: {}", rpc_url).as_str()),
                        );

                        let token_address = chain_specific_config.address.clone().parse().expect(
                            format!("Invalid address: {}", chain_specific_config.address).as_str(),
                        );

                        let token = ERC20ContractInstance::new(token_address, provider);

                        ((*chain_id, &chain_specific_config.address), token)
                    })
                    .collect::<Vec<_>>()
            })
            .collect();

        let settlement_engine =
            SettlementEngine::new(&config, bungee_client, client, erc20_instance_map);

        return settlement_engine;
    }

    const TEST_OWNER_WALLET: &str = "0xe0E67a6F478D7ED604Cf528bDE6C3f5aB034b59D";
    const TOKEN_ADDRESS_USDC_42161: &str = "0xaf88d065e77c8cC2239327C5EDb3A432268e5831";
    // Target has currently 0 approval for token on arbitrum mainnet for USDC token
    const TARGET_NO_APPROVAL_BY_OWNER_ON_42161_FOR_USDC: &str =
        "0x22f966A213288B29bB1F650a923E8f70dAd2515A";
    // Target has 100 approval for token on arbitrum mainnet for USDC token
    const TARGET_100_USDC_APPROVAL_BY_OWNER_ON_42161_FOR_USDC: &str =
        "0xE4ec34b790e2fCabF37Cc9938A34327ddEadDc78";

    #[tokio::test]
    async fn test_should_generate_required_approval_transaction() {
        let config = setup_config();
        let engine = setup(&config);

        let required_approval_data = RequiredApprovalDetails {
            chain_id: 42161,
            token_address: TOKEN_ADDRESS_USDC_42161.to_string(),
            owner: TEST_OWNER_WALLET.to_string(),
            target: TARGET_NO_APPROVAL_BY_OWNER_ON_42161_FOR_USDC.to_string(),
            amount: U256::from(100),
        };

        let transaction = engine
            .generate_transaction_for_approval(&required_approval_data)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(transaction.transaction_type, TransactionType::Approval);
        assert_eq!(transaction.transaction.to, TOKEN_ADDRESS_USDC_42161);
        assert_eq!(transaction.transaction.value, U256::ZERO);
        assert_eq!(
            transaction.transaction.calldata,
            engine
                .erc20_instance_map
                .get(&(required_approval_data.chain_id, &required_approval_data.token_address))
                .expect("ERC20 Utils not found")
                .approve(
                    TARGET_NO_APPROVAL_BY_OWNER_ON_42161_FOR_USDC
                        .to_string()
                        .parse()
                        .expect("Invalid address"),
                    U256::from(100)
                )
                .calldata()
                .to_string()
        )
    }

    #[tokio::test]
    async fn test_should_take_existing_approval_into_consideration_while_building_approval_data() {
        let config = setup_config();
        let engine = setup(&config);

        let required_approval_data = RequiredApprovalDetails {
            chain_id: 42161,
            token_address: TOKEN_ADDRESS_USDC_42161.to_string(),
            owner: TEST_OWNER_WALLET.to_string(),
            target: TARGET_100_USDC_APPROVAL_BY_OWNER_ON_42161_FOR_USDC.to_string(),
            amount: U256::from(150),
        };

        let transaction = engine
            .generate_transaction_for_approval(&required_approval_data)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(transaction.transaction_type, TransactionType::Approval);
        assert_eq!(transaction.transaction.to, TOKEN_ADDRESS_USDC_42161);
        assert_eq!(transaction.transaction.value, U256::ZERO);
        assert_eq!(
            transaction.transaction.calldata,
            engine
                .erc20_instance_map
                .get(&(required_approval_data.chain_id, &required_approval_data.token_address))
                .expect("ERC20 Utils not found")
                .approve(
                    TARGET_100_USDC_APPROVAL_BY_OWNER_ON_42161_FOR_USDC
                        .to_string()
                        .parse()
                        .expect("Invalid address"),
                    U256::from(50)
                )
                .calldata()
                .to_string()
        )
    }

    #[tokio::test]
    async fn test_should_generate_approvals_for_multiple_required_transactions() {
        let config = setup_config();
        let engine = setup(&config);

        let required_approval_datas = vec![
            RequiredApprovalDetails {
                chain_id: 42161,
                token_address: TOKEN_ADDRESS_USDC_42161.to_string(),
                owner: TEST_OWNER_WALLET.to_string(),
                target: TARGET_NO_APPROVAL_BY_OWNER_ON_42161_FOR_USDC.to_string(),
                amount: U256::from(100),
            },
            RequiredApprovalDetails {
                chain_id: 42161,
                token_address: TOKEN_ADDRESS_USDC_42161.to_string(),
                owner: TEST_OWNER_WALLET.to_string(),
                target: TARGET_100_USDC_APPROVAL_BY_OWNER_ON_42161_FOR_USDC.to_string(),
                amount: U256::from(150),
            },
        ];

        let mut transactions =
            engine.generate_transactions_for_approvals(&required_approval_datas).await.unwrap();
        transactions.sort_by(|a, b| a.transaction.calldata.cmp(&b.transaction.calldata));

        assert_eq!(transactions[0].transaction_type, TransactionType::Approval);
        assert_eq!(transactions[0].transaction.to, TOKEN_ADDRESS_USDC_42161);
        assert_eq!(transactions[0].transaction.value, U256::ZERO);
        assert_eq!(
            transactions[0].transaction.calldata,
            engine
                .erc20_instance_map
                .get(&(
                    required_approval_datas[0].chain_id,
                    &required_approval_datas[0].token_address
                ))
                .expect("ERC20 Utils not found")
                .approve(
                    TARGET_NO_APPROVAL_BY_OWNER_ON_42161_FOR_USDC
                        .to_string()
                        .parse()
                        .expect("Invalid address"),
                    U256::from(100)
                )
                .calldata()
                .to_string()
        );

        assert_eq!(transactions[1].transaction_type, TransactionType::Approval);
        assert_eq!(transactions[1].transaction.to, TOKEN_ADDRESS_USDC_42161);
        assert_eq!(transactions[1].transaction.value, U256::ZERO);
        assert_eq!(
            transactions[1].transaction.calldata,
            engine
                .erc20_instance_map
                .get(&(
                    required_approval_datas[1].chain_id,
                    &required_approval_datas[1].token_address
                ))
                .expect("ERC20 Utils not found")
                .approve(
                    TARGET_100_USDC_APPROVAL_BY_OWNER_ON_42161_FOR_USDC
                        .to_string()
                        .parse()
                        .expect("Invalid address"),
                    U256::from(50)
                )
                .calldata()
                .to_string()
        );
    }

    #[tokio::test]
    async fn test_should_merge_approvals_from_same_owner_to_same_spender_for_same_token_and_chain()
    {
        let config = setup_config();
        let engine = setup(&config);

        let required_approval_datas = vec![
            RequiredApprovalDetails {
                chain_id: 42161,
                token_address: TOKEN_ADDRESS_USDC_42161.to_string(),
                owner: TEST_OWNER_WALLET.to_string(),
                target: TARGET_NO_APPROVAL_BY_OWNER_ON_42161_FOR_USDC.to_string(),
                amount: U256::from(100),
            },
            RequiredApprovalDetails {
                chain_id: 42161,
                token_address: TOKEN_ADDRESS_USDC_42161.to_string(),
                owner: TEST_OWNER_WALLET.to_string(),
                target: TARGET_NO_APPROVAL_BY_OWNER_ON_42161_FOR_USDC.to_string(),
                amount: U256::from(150),
            },
        ];

        let transactions =
            engine.generate_transactions_for_approvals(&required_approval_datas).await.unwrap();

        assert_eq!(transactions.len(), 1);
        assert_eq!(transactions[0].transaction_type, TransactionType::Approval);
        assert_eq!(transactions[0].transaction.to, TOKEN_ADDRESS_USDC_42161);
        assert_eq!(transactions[0].transaction.value, U256::ZERO);
        assert_eq!(
            transactions[0].transaction.calldata,
            engine
                .erc20_instance_map
                .get(&(
                    required_approval_datas[0].chain_id,
                    &required_approval_datas[0].token_address
                ))
                .expect("ERC20 Utils not found")
                .approve(
                    TARGET_NO_APPROVAL_BY_OWNER_ON_42161_FOR_USDC
                        .to_string()
                        .parse()
                        .expect("Invalid address"),
                    U256::from(250)
                )
                .calldata()
                .to_string()
        );
    }

    #[tokio::test]
    async fn test_should_generate_transactions_for_bridging_routes() {
        let config = setup_config();
        let engine = setup(&config);

        let bridge_result = BridgeResult::build(
            &config,
            &1,
            &42161,
            &"USDC".to_string(),
            &"USDC".to_string(),
            false,
            100.0,
            TEST_OWNER_WALLET.to_string(),
            TEST_OWNER_WALLET.to_string(),
        )
        .expect("Failed to build bridge result");

        let transactions = engine
            .generate_transactions(&vec![bridge_result])
            .await
            .expect("Failed to generate transactions");

        assert_eq!(transactions.len(), 2);
        assert_eq!(transactions[0].transaction_type, TransactionType::Approval);
        assert_eq!(transactions[1].transaction_type, TransactionType::Bungee);
    }

    #[tokio::test]
    async fn test_should_generate_transactions_for_swaps() {
        let config = setup_config();
        let engine = setup(&config);

        let bridge_result = BridgeResult::build(
            &config,
            &1,
            &1,
            &"USDC".to_string(),
            &"USDT".to_string(),
            false,
            100.0,
            TEST_OWNER_WALLET.to_string(),
            TEST_OWNER_WALLET.to_string(),
        )
        .expect("Failed to build bridge result");

        let transactions = engine
            .generate_transactions(&vec![bridge_result])
            .await
            .expect("Failed to generate transactions");

        assert_eq!(transactions.len(), 2);
        assert_eq!(transactions[0].transaction_type, TransactionType::Approval);
        assert_eq!(transactions[1].transaction_type, TransactionType::Bungee);
    }
}
