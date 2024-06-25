use std::error::Error;
use std::fmt::Debug;

use ruint::aliases::U256;

pub use bungee::BungeeClient;

use crate::{CostType, Route};

pub mod bungee;

type Calldata = String;

pub trait RouteSource: Debug {
    type FetchRouteCostError: Debug + Error;
    type GenerateRouteCalldataError: Debug + Error;

    async fn fetch_least_route_cost_in_usd(
        &self,
        route: &Route,
        from_token_amount: U256,
        estimation_type: &CostType,
    ) -> Result<f64, Self::FetchRouteCostError>;

    async fn generate_route_calldata(
        &self,
        route: &Route,
    ) -> Result<Calldata, Self::GenerateRouteCalldataError>;
}
