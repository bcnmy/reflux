use std::collections::HashMap;
use std::sync::Arc;

use alloy::hex::FromHexError;
use alloy::providers::{ProviderBuilder, RootProvider};
use alloy::transports::http::Http;
use futures::StreamExt;
use log::{error, info};
use reqwest::{Client, Url};
use ruint::aliases::U256;
use ruint::Uint;
use serde::Serialize;
use thiserror::Error;
use tokio::sync::Mutex;

use config::{Config, TokenAddress};

use crate::{blockchain, BridgeResult, BridgeResultVecWrapper, Route};
use crate::blockchain::erc20::IERC20::IERC20Instance;
use crate::source::{EthereumTransaction, RequiredApprovalDetails, RouteSource};
use crate::token_price::TokenPriceProvider;
use crate::token_price::utils::{Errors, get_token_amount_from_value_in_usd};

pub struct SettlementEngine<Source: RouteSource, PriceProvider: TokenPriceProvider> {
    source: Source,
    config: Arc<Config>,
    price_provider: Arc<Mutex<PriceProvider>>,
    // (chain_id, token_address) -> contract
    erc20_instance_map: HashMap<(u32, TokenAddress), blockchain::ERC20Contract>,
}

#[derive(Debug, PartialEq, Serialize)]
pub enum TransactionType {
    Approval(ApprovalDetails),
    Bungee(BungeeDetails),
}

#[derive(Debug, Serialize, PartialEq)]
pub struct BungeeDetails {
    pub from_chain: u32,
    pub to_chain: u32,
    pub from_address: String,
    pub to_address: String,
    pub token: String,
    pub token_amount: U256,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct ApprovalDetails {
    pub chain: u32,
    pub token: String,
    pub token_amount: U256,
}

#[derive(Debug, Serialize)]
pub struct TransactionWithType {
    transaction: EthereumTransaction,
    transaction_type: TransactionType,
}

const GENERATE_TRANSACTIONS_CONCURRENCY: usize = 10;

impl<Source: RouteSource, PriceProvider: TokenPriceProvider>
    SettlementEngine<Source, PriceProvider>
{
    pub fn new(
        config: Arc<Config>,
        source: Source,
        price_provider: Arc<Mutex<PriceProvider>>,
        erc20_instance_map: HashMap<(u32, TokenAddress), blockchain::ERC20Contract>,
    ) -> Self {
        SettlementEngine { source, config, price_provider, erc20_instance_map }
    }

    pub async fn generate_transactions(
        &self,
        routes: Vec<BridgeResult>,
    ) -> Result<Vec<TransactionWithType>, SettlementEngineErrors<Source, PriceProvider>> {
        info!("Generating transactions for routes: {}", BridgeResultVecWrapper(&routes));

        let (results, failed): (
            Vec<
                Result<
                    (Vec<(BungeeDetails, EthereumTransaction)>, Vec<RequiredApprovalDetails>),
                    SettlementEngineErrors<Source, PriceProvider>,
                >,
            >,
            _,
        ) = futures::stream::iter(routes.into_iter())
            .map(|route| async move {
                info!("Generating transactions for route: {}", route.route);

                let token_amount = get_token_amount_from_value_in_usd(
                    &self.config,
                    &self.price_provider.lock().await,
                    &route.route.from_token.symbol,
                    route.route.from_chain.id,
                    &route.source_amount_in_usd,
                )
                .await
                .map_err(|err| SettlementEngineErrors::GetTokenAmountFromValueInUsdError(err))?;

                info!("Token amount: {:?} for route {}", token_amount, route);

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

                info!("Generated transactions: {:?} for route {}", ethereum_transactions, route);

                Ok::<_, SettlementEngineErrors<_, _>>((
                    ethereum_transactions
                        .into_iter()
                        .map(|t| {
                            (
                                BungeeDetails {
                                    from_chain: route.route.from_chain.id,
                                    to_chain: route.route.to_chain.id,
                                    from_address: route.from_address.clone(),
                                    to_address: route.to_address.clone(),
                                    token: route.route.from_token.symbol.clone(),
                                    token_amount: token_amount.clone(),
                                },
                                t,
                            )
                        })
                        .collect(),
                    required_approval_details,
                ))
            })
            .buffer_unordered(GENERATE_TRANSACTIONS_CONCURRENCY)
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .partition(Result::is_ok);

        let mut failed: Vec<_> = failed.into_iter().map(Result::unwrap_err).collect();
        if !failed.is_empty() {
            error!("Failed to generate transactions: {:?}", failed);
            return Err(failed.remove(0));
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
            .map(|(details, t)| TransactionWithType {
                transaction: t,
                transaction_type: TransactionType::Bungee(details),
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

        let token_instance = self.erc20_instance_map.get(&(
            required_approval_details.chain_id,
            required_approval_details.token_address.clone(),
        ));

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

        let required_approval = required_approval_details.amount;
        info!(
            "Required Approval: {:?} against requirement: {:?}",
            required_approval, required_approval_details
        );

        let calldata = token_instance.approve(spender, required_approval).calldata().to_string();

        Ok(Some(TransactionWithType {
            transaction: EthereumTransaction {
                from_address: required_approval_details.owner.clone(),
                from_chain: required_approval_details.chain_id,
                to: token_instance.address().to_string(),
                value: Uint::ZERO,
                calldata,
            },
            transaction_type: TransactionType::Approval(ApprovalDetails {
                chain: required_approval_details.chain_id,
                token: required_approval_details.token_address.to_string(),
                token_amount: required_approval,
            }),
        }))
    }

    async fn generate_transactions_for_approvals(
        &self,
        approvals: &Vec<RequiredApprovalDetails>,
    ) -> Result<Vec<TransactionWithType>, SettlementEngineErrors<Source, PriceProvider>> {
        info!("Generating transactions for approvals: {:?}", approvals);

        // Group the approvals and combine them based on chain_id, token_address, spender and target
        let mut approvals_grouped =
            HashMap::<(u32, &TokenAddress, &String, &String), Vec<&RequiredApprovalDetails>>::new();
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
    ERC20UtilsNotFound(u32, TokenAddress),

    #[error("Error parsing address: {0}")]
    InvalidAddressError(FromHexError),

    #[error("Error while calling ERC20 Contract: {0}")]
    AlloyError(#[from] alloy::contract::Error),
}

pub fn generate_erc20_instance_map(
    config: &Config,
) -> Result<
    HashMap<(u32, TokenAddress), blockchain::ERC20Contract>,
    Vec<GenerateERC20InstanceMapErrors>,
> {
    let (result, failed): (Vec<_>, _) = config
        .tokens
        .iter()
        .flat_map(|(_, token)| {
            token
                .by_chain
                .iter()
                .map(
                    |(chain_id, chain_specific_config)| -> Result<
                        (
                            (u32, TokenAddress),
                            IERC20Instance<Http<Client>, RootProvider<Http<Client>>>,
                        ),
                        GenerateERC20InstanceMapErrors,
                    > {
                        let rpc_url = &config
                            .chains
                            .iter()
                            .find(|(&id, _)| id == *chain_id)
                            .ok_or(GenerateERC20InstanceMapErrors::ChainNoFound(*chain_id))?
                            .1
                            .rpc_url;

                        let provider =
                            ProviderBuilder::new().on_http(Url::parse(rpc_url.as_str()).map_err(
                                |_| GenerateERC20InstanceMapErrors::InvalidRPCUrl(rpc_url.clone()),
                            )?);

                        let token_address = chain_specific_config.address.clone();

                        let token = blockchain::ERC20Contract::new(
                            token_address.to_string().parse().map_err(|err| {
                                GenerateERC20InstanceMapErrors::InvalidAddressError(
                                    token_address.clone().to_string(),
                                    err,
                                )
                            })?,
                            provider,
                        );

                        Ok(((*chain_id, chain_specific_config.address.clone()), token))
                    },
                )
                .collect::<Vec<_>>()
        })
        .partition(Result::is_ok);

    if failed.len() != 0 {
        return Err(failed.into_iter().map(Result::unwrap_err).collect());
    }

    Ok(result.into_iter().map(Result::unwrap).collect())
}

#[derive(Error, Debug)]
pub enum GenerateERC20InstanceMapErrors {
    #[error("Chain not found for id: {0}")]
    ChainNoFound(u32),

    #[error("Error parsing RPC URL: {0}")]
    InvalidRPCUrl(String),

    #[error("Error parsing address: {0}")]
    InvalidAddressError(String, FromHexError),
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::env;
    use std::sync::Arc;
    use std::time::Duration;

    use alloy::primitives::U256;
    use async_trait::async_trait;
    use derive_more::Display;
    use thiserror::Error;
    use tokio::sync::Mutex;

    use config::{Config, get_sample_config};
    use storage::{KeyValueStore, RedisClientError};

    use crate::{BridgeResult, BungeeClient, CoingeckoClient};
    use crate::settlement_engine::{
        generate_erc20_instance_map, SettlementEngine, SettlementEngineErrors, TransactionType,
    };
    use crate::source::{EthereumTransaction, RequiredApprovalDetails};

    #[derive(Error, Debug, Display)]
    struct Err;

    #[derive(Default, Debug)]
    struct KVStore {
        map: Mutex<HashMap<String, String>>,
    }

    #[async_trait]
    impl KeyValueStore for KVStore {
        type Error = Err;

        async fn get(&self, k: &String) -> Result<String, Self::Error> {
            match self.map.lock().await.get(k) {
                Some(v) => Ok(v.clone()),
                None => Result::Err(Err),
            }
        }

        async fn get_multiple(&self, _: &Vec<String>) -> Result<Vec<String>, Self::Error> {
            unimplemented!()
        }

        async fn set(&self, k: &String, v: &String, _: Duration) -> Result<(), Self::Error> {
            self.map
                .lock()
                .await
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

    fn setup(config: &Arc<Config>) -> SettlementEngine<BungeeClient, CoingeckoClient<KVStore>> {
        let bungee_client = BungeeClient::new(
            &"https://api.socket.tech/v2".to_string(),
            &env::var("BUNGEE_API_KEY").unwrap().to_string(),
        )
        .unwrap();

        let client = Arc::new(Mutex::new(CoingeckoClient::new(
            config.coingecko.base_url.clone(),
            env::var("COINGECKO_API_KEY").unwrap(),
            KVStore::default(),
            Duration::from_secs(config.coingecko.expiry_sec),
        )));

        let erc20_instance_map = generate_erc20_instance_map(&config).unwrap();

        let settlement_engine =
            SettlementEngine::new(Arc::clone(config), bungee_client, client, erc20_instance_map);

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
        let config = Arc::new(setup_config());
        let engine = setup(&config);

        let required_approval_data = RequiredApprovalDetails {
            chain_id: 42161,
            token_address: TOKEN_ADDRESS_USDC_42161.to_string().try_into().unwrap(),
            owner: TEST_OWNER_WALLET.to_string(),
            target: TARGET_NO_APPROVAL_BY_OWNER_ON_42161_FOR_USDC.to_string(),
            amount: U256::from(100),
        };

        let transaction = engine
            .generate_transaction_for_approval(&required_approval_data)
            .await
            .unwrap()
            .unwrap();

        assert!(if let TransactionType::Approval(_) = transaction.transaction_type {
            true
        } else {
            false
        });
        assert_eq!(transaction.transaction.to, TOKEN_ADDRESS_USDC_42161);
        assert_eq!(transaction.transaction.value, U256::ZERO);
        assert_eq!(
            transaction.transaction.calldata,
            engine
                .erc20_instance_map
                .get(&(required_approval_data.chain_id, required_approval_data.token_address))
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
        let config = Arc::new(setup_config());
        let engine = setup(&config);

        let required_approval_data = RequiredApprovalDetails {
            chain_id: 42161,
            token_address: TOKEN_ADDRESS_USDC_42161.to_string().try_into().unwrap(),
            owner: TEST_OWNER_WALLET.to_string(),
            target: TARGET_100_USDC_APPROVAL_BY_OWNER_ON_42161_FOR_USDC.to_string(),
            amount: U256::from(150),
        };

        let transaction = engine
            .generate_transaction_for_approval(&required_approval_data)
            .await
            .unwrap()
            .unwrap();

        assert!(if let TransactionType::Approval(_) = transaction.transaction_type {
            true
        } else {
            false
        });

        assert_eq!(transaction.transaction.to, TOKEN_ADDRESS_USDC_42161);
        assert_eq!(transaction.transaction.value, U256::ZERO);
        assert_eq!(
            transaction.transaction.calldata,
            engine
                .erc20_instance_map
                .get(&(required_approval_data.chain_id, required_approval_data.token_address))
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
        let config = Arc::new(setup_config());
        let engine = setup(&config);

        let required_approval_datas = vec![
            RequiredApprovalDetails {
                chain_id: 42161,
                token_address: TOKEN_ADDRESS_USDC_42161.to_string().try_into().unwrap(),
                owner: TEST_OWNER_WALLET.to_string(),
                target: TARGET_NO_APPROVAL_BY_OWNER_ON_42161_FOR_USDC.to_string(),
                amount: U256::from(100),
            },
            RequiredApprovalDetails {
                chain_id: 42161,
                token_address: TOKEN_ADDRESS_USDC_42161.to_string().try_into().unwrap(),
                owner: TEST_OWNER_WALLET.to_string(),
                target: TARGET_100_USDC_APPROVAL_BY_OWNER_ON_42161_FOR_USDC.to_string(),
                amount: U256::from(150),
            },
        ];

        let mut transactions =
            engine.generate_transactions_for_approvals(&required_approval_datas).await.unwrap();
        transactions.sort_by(|a, b| a.transaction.calldata.cmp(&b.transaction.calldata));

        assert!(if let TransactionType::Approval(_) = transactions[0].transaction_type {
            true
        } else {
            false
        });
        assert_eq!(transactions[0].transaction.to, TOKEN_ADDRESS_USDC_42161);
        assert_eq!(transactions[0].transaction.value, U256::ZERO);
        assert_eq!(
            transactions[0].transaction.calldata,
            engine
                .erc20_instance_map
                .get(&(
                    required_approval_datas[0].chain_id,
                    required_approval_datas[0].token_address.clone()
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

        assert!(if let TransactionType::Approval(_) = transactions[1].transaction_type {
            true
        } else {
            false
        });
        assert_eq!(transactions[1].transaction.to, TOKEN_ADDRESS_USDC_42161);
        assert_eq!(transactions[1].transaction.value, U256::ZERO);
        assert_eq!(
            transactions[1].transaction.calldata,
            engine
                .erc20_instance_map
                .get(&(
                    required_approval_datas[1].chain_id,
                    required_approval_datas[1].token_address.clone()
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
        let config = Arc::new(setup_config());
        let engine = setup(&config);

        let required_approval_datas = vec![
            RequiredApprovalDetails {
                chain_id: 42161,
                token_address: TOKEN_ADDRESS_USDC_42161.to_string().try_into().unwrap(),
                owner: TEST_OWNER_WALLET.to_string(),
                target: TARGET_NO_APPROVAL_BY_OWNER_ON_42161_FOR_USDC.to_string(),
                amount: U256::from(100),
            },
            RequiredApprovalDetails {
                chain_id: 42161,
                token_address: TOKEN_ADDRESS_USDC_42161.to_string().try_into().unwrap(),
                owner: TEST_OWNER_WALLET.to_string(),
                target: TARGET_NO_APPROVAL_BY_OWNER_ON_42161_FOR_USDC.to_string(),
                amount: U256::from(150),
            },
        ];

        let transactions =
            engine.generate_transactions_for_approvals(&required_approval_datas).await.unwrap();

        assert_eq!(transactions.len(), 1);
        assert!(if let TransactionType::Approval(_) = transactions[0].transaction_type {
            true
        } else {
            false
        });
        assert_eq!(transactions[0].transaction.to, TOKEN_ADDRESS_USDC_42161);
        assert_eq!(transactions[0].transaction.value, U256::ZERO);
        assert_eq!(
            transactions[0].transaction.calldata,
            engine
                .erc20_instance_map
                .get(&(
                    required_approval_datas[0].chain_id,
                    required_approval_datas[0].token_address.clone()
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
        let config = Arc::new(setup_config());
        let engine = setup(&config);

        let bridge_result = BridgeResult::build(
            &config,
            &42161,
            &10,
            &"USDC".to_string(),
            &"USDC".to_string(),
            false,
            1000.0,
            TEST_OWNER_WALLET.to_string(),
            TEST_OWNER_WALLET.to_string(),
        )
        .expect("Failed to build bridge result");

        let transactions = engine
            .generate_transactions(vec![bridge_result])
            .await
            .expect("Failed to generate transactions");

        assert_eq!(transactions.len(), 2);

        assert!(if let TransactionType::Approval(_) = transactions[0].transaction_type {
            true
        } else {
            false
        });
        assert!(if let TransactionType::Bungee(_) = transactions[1].transaction_type {
            true
        } else {
            false
        });
    }

    #[tokio::test]
    async fn test_should_generate_transactions_for_swaps() {
        let config = Arc::new(setup_config());
        let engine = setup(&config);

        let bridge_result = BridgeResult::build(
            &config,
            &42161,
            &10,
            &"USDC".to_string(),
            &"USDT".to_string(),
            false,
            1000.0,
            TEST_OWNER_WALLET.to_string(),
            TEST_OWNER_WALLET.to_string(),
        )
        .expect("Failed to build bridge result");

        let transactions = engine
            .generate_transactions(vec![bridge_result])
            .await
            .expect("Failed to generate transactions");

        assert_eq!(transactions.len(), 2);
        assert!(if let TransactionType::Approval(_) = transactions[0].transaction_type {
            true
        } else {
            false
        });
        assert!(if let TransactionType::Bungee(_) = transactions[1].transaction_type {
            true
        } else {
            false
        });
    }

    fn assert_is_send<T: Send>() {}

    #[test]
    fn test_custom_types_must_be_send() {
        assert_is_send::<SettlementEngine<BungeeClient, CoingeckoClient<KVStore>>>();
        assert_is_send::<EthereumTransaction>();
        assert_is_send::<RequiredApprovalDetails>();
        assert_is_send::<SettlementEngineErrors<BungeeClient, CoingeckoClient<KVStore>>>();
        assert_is_send::<U256>();
        assert_is_send::<
            Result<
                (Vec<EthereumTransaction>, Vec<RequiredApprovalDetails>),
                SettlementEngineErrors<BungeeClient, CoingeckoClient<KVStore>>,
            >,
        >()
    }
}
