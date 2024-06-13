use num_bigint::BigUint;

pub use bungee::BungeeClient;
use config;

use crate::{EstimationType, Route};

mod bungee;

type Calldata = String;

trait RouteSource {
    type FetchRouteCostError;
    type GenerateRouteCalldataError;

    async fn fetch_route_cost_in_usd(
        &self,
        route: &Route,
        from_token_amount: BigUint,
        estimation_type: EstimationType,
    ) -> Result<f64, Self::FetchRouteCostError>;

    async fn generate_route_calldata(
        &self,
        route: &Route,
    ) -> Result<Calldata, Self::GenerateRouteCalldataError>;
}
