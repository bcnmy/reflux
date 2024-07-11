use std::error::Error;
use std::fmt::Debug;

use ruint::aliases::U256;
use serde::Serialize;

use crate::{CostType, Route};

pub mod bungee;

#[derive(Debug, Serialize)]
pub struct EthereumTransaction {
    pub from: String,
    pub to: String,
    pub value: U256,
    pub calldata: String,
}

#[derive(Debug, Clone)]
pub struct RequiredApprovalDetails {
    pub chain_id: u32,
    pub token_address: String,
    pub owner: String,
    pub target: String,
    pub amount: U256,
}

pub trait RouteSource: Debug + Send + Sync {
    type FetchRouteCostError: Debug + Error + Send + Sync;
    type GenerateRouteTransactionsError: Debug + Error + Send + Sync;
    type BaseRouteType: Debug + Send + Sync;

    fn fetch_least_cost_route_and_cost_in_usd(
        &self,
        route: &Route,
        from_token_amount: &U256,
        sender_address: Option<&String>,
        recipient_address: Option<&String>,
        estimation_type: &CostType,
    ) -> impl futures::Future<Output = Result<(Self::BaseRouteType, f64), Self::FetchRouteCostError>>;

    fn generate_route_transactions(
        &self,
        route: &Route,
        amount: &U256,
        sender_address: &String,
        recipient_address: &String,
    ) -> impl futures::Future<
        Output = Result<
            (Vec<EthereumTransaction>, Vec<RequiredApprovalDetails>),
            Self::GenerateRouteTransactionsError,
        >,
    >;
}
