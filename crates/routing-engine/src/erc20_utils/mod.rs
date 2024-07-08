use std::error::Error;
use std::fmt::Debug;
use std::future::Future;

use crate::settlement_engine::TransactionWithType;
use crate::source::RequiredApprovalDetails;

pub trait ERC20Utils {
    type Error: Error + Debug;

    fn generate_required_approval_transaction(
        &self,
        required_approval_details: &RequiredApprovalDetails,
    ) -> impl Future<Output = Result<TransactionWithType, Self::Error>>;
}
