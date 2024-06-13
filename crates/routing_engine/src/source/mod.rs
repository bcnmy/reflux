use ruint::aliases::U256;

pub use bungee::BungeeClient;
use config;

use crate::{CostType, Route};

mod bungee;

type Calldata = String;

pub(crate) trait RouteSource {
    type FetchRouteCostError;
    type GenerateRouteCalldataError;

    async fn fetch_least_route_cost_in_usd(
        &self,
        route: &Route,
        from_token_amount: U256,
        estimation_type: CostType,
    ) -> Result<f64, Self::FetchRouteCostError>;

    async fn generate_route_calldata(
        &self,
        route: &Route,
    ) -> Result<Calldata, Self::GenerateRouteCalldataError>;
}
