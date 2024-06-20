use async_trait::async_trait;
use mockall::predicate::*;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        engine::{Route, RoutingEngine},
        traits::{AccountAggregation, RouteFee},
    };
    use account_aggregation::types::Balance;
    use mockall::mock;

    mock! {
        pub AccountAggregationService {}

        #[async_trait]
        impl AccountAggregation for AccountAggregationService {
            async fn get_user_accounts_balance(&self, user_id: &String) -> Result<Vec<Balance>, Box<dyn std::error::Error>>;
        }
    }

    mock! {
        pub RouteFeeBucket {}

        impl RouteFee for RouteFeeBucket {
            fn get_cost(&self, from_chain: u32, to_chain: u32, from_token: &str, to_token: &str) -> (f64, f64, String);
        }
    }

    #[tokio::test]
    async fn test_get_best_cost_path() {
        let mut mock_aas = MockAccountAggregationService::new();
        let mut mock_rfb = MockRouteFeeBucket::new();

        mock_aas.expect_get_user_accounts_balance().returning(|_user_id| {
            Ok(vec![
                Balance {
                    token: "USDC".to_string(),
                    token_address: "USDC".to_string(),
                    chain_id: 137,
                    amount: 50.0,
                    amount_in_usd: 50.0,
                },
                Balance {
                    token: "USDC".to_string(),
                    token_address: "USDC".to_string(),
                    chain_id: 42161,
                    amount: 25.0,
                    amount_in_usd: 25.0,
                },
                Balance {
                    token: "ETH".to_string(),
                    token_address: "ETH".to_string(),
                    chain_id: 137,
                    amount: 0.1,
                    amount_in_usd: 200.0,
                },
            ])
        });

        mock_rfb
            .expect_get_cost()
            .withf(|from_chain, to_chain, from_token, to_token| {
                *from_chain == 137 && *to_chain == 1 && from_token == "USDC" && to_token == "USDC"
            })
            .returning(|_from_chain, _to_chain, _from_token, _to_token| {
                (0.1, 50.0, "BridgeA".to_string())
            });

        mock_rfb
            .expect_get_cost()
            .withf(|from_chain, to_chain, from_token, to_token| {
                *from_chain == 42161 && *to_chain == 1 && from_token == "USDC" && to_token == "USDC"
            })
            .returning(|_from_chain, _to_chain, _from_token, _to_token| {
                (0.2, 25.0, "BridgeB".to_string())
            });

        // let routing_engine = RoutingEngine::new(mock_aas);

        // let user_id = "user1";
        // let to_chain = 1;
        // let to_token = "USDC";
        // let to_value = 60.0;

        // let routes = routing_engine.get_best_cost_path(user_id, to_chain, to_token, to_value).await;
        // println!("{:?}", routes);

        // let expected_routes = vec![
        //     Route {
        //         from_chain: 137,
        //         to_chain: 1,
        //         token: "USDC".to_string(),
        //         amount: 50.0,
        //         path: vec!["BridgeA".to_string()],
        //     },
        //     Route {
        //         from_chain: 42161,
        //         to_chain: 1,
        //         token: "USDC".to_string(),
        //         amount: 10.0,
        //         path: vec!["BridgeB".to_string()],
        //     },
        // ];

        // assert_eq!(routes, expected_routes);
    }

    #[tokio::test]
    async fn test_get_best_cost_path_with_swap() {
        // let mut mock_aas = MockAccountAggregationService::new();
        // let mut mock_rfb = MockRouteFeeBucket::new();

        // mock_aas
        //     .expect_get_user_accounts_balance()
        //     .returning(|_user_id| {
        //         Ok(vec![
        //             Balance {
        //                 token: "USDC".to_string(),
        //                 token_address: "USDC".to_string(),
        //                 chain_id: 137,
        //                 amount: 50.0,
        //                 amount_in_usd: 50.0,
        //             },
        //             Balance {
        //                 token: "USDC".to_string(),
        //                 token_address: "USDC".to_string(),
        //                 chain_id: 42161,
        //                 amount: 25.0,
        //                 amount_in_usd: 25.0,
        //             },
        //             Balance {
        //                 token: "ETH".to_string(),
        //                 token_address: "ETH".to_string(),
        //                 chain_id: 137,
        //                 amount: 0.1,
        //                 amount_in_usd: 200.0,
        //             },
        //         ])
        //     });

        // mock_rfb
        //     .expect_get_cost()
        //     .withf(|from_chain, to_chain, from_token, to_token| {
        //         *from_chain == 137 && *to_chain == 1 && from_token == "USDC" && to_token == "USDC"
        //     })
        //     .returning(|_from_chain, _to_chain, _from_token, _to_token| {
        //         (0.1, 50.0, "BridgeA".to_string())
        //     });

        // mock_rfb
        //     .expect_get_cost()
        //     .withf(|from_chain, to_chain, from_token, to_token| {
        //         *from_chain == 42161 && *to_chain == 1 && from_token == "USDC" && to_token == "USDC"
        //     })
        //     .returning(|_from_chain, _to_chain, _from_token, _to_token| {
        //         (0.2, 25.0, "BridgeB".to_string())
        //     });

        // mock_rfb
        //     .expect_get_cost()
        //     .withf(|from_chain, to_chain, from_token, to_token| {
        //         *from_chain == 137 && *to_chain == 1 && from_token == "ETH" && to_token == "USDC"
        //     })
        //     .returning(|_from_chain, _to_chain, _from_token, _to_token| {
        //         (0.5, 200.0, "BridgeC".to_string())
        //     });

        // let routing_engine = RoutingEngine::new(mock_aas);

        // let user_id = "user1";
        // let to_chain = 1;
        // let to_token = "USDC";
        // let to_value = 80.0;

        // let routes = routing_engine.get_best_cost_path(user_id, to_chain, to_token, to_value).await;
        // println!("{:?}", routes);

        // let expected_routes = vec![
        //     Route {
        //         from_chain: 137,
        //         to_chain: 1,
        //         token: "USDC".to_string(),
        //         amount: 50.0,
        //         path: vec!["BridgeA".to_string()],
        //     },
        //     Route {
        //         from_chain: 42161,
        //         to_chain: 1,
        //         token: "USDC".to_string(),
        //         amount: 25.0,
        //         path: vec!["BridgeB".to_string()],
        //     },
        //     Route {
        //         from_chain: 137,
        //         to_chain: 1,
        //         token: "ETH".to_string(),
        //         amount: 0.025,
        //         path: vec!["BridgeC".to_string()],
        //     },
        // ];

        // assert_eq!(routes, expected_routes);
    }
}
