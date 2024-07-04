use thiserror::Error;

use crate::BridgeResult;
use crate::source::{EthereumTransaction, RouteSource};

pub struct SettlementEngine<Source: RouteSource> {
    source: Source,
}

pub enum TransactionType {
    Approval,
    BungeeBridge,
}

impl<'config, Source: RouteSource> SettlementEngine<Source> {
    pub fn new(source: Source) -> Self {
        SettlementEngine { source }
    }

    async fn generate_transactions(
        &self,
        routes: Vec<BridgeResult<'config>>,
    ) -> Result<Vec<EthereumTransaction>, SettlementEngineErrors> {
        todo!()
    }
}

#[derive(Error, Debug)]
pub enum SettlementEngineErrors {}
