use std::error::Error;
use std::fmt::Debug;

use serde::{Deserialize, Serialize};

pub use linear_regression_estimator::LinearRegressionEstimator;

pub mod linear_regression_estimator;

#[derive(Debug)]
pub struct DataPoint<Input, Output> {
    pub(crate) x: Input,
    pub(crate) y: Output,
}

pub trait Estimator<'de, Input, Output>: Serialize + Deserialize<'de> + Debug {
    type Error: Error + Debug;

    fn build(data: Vec<DataPoint<Input, Output>>) -> Result<Self, Self::Error>;

    fn estimate(&self, x: Input) -> Output;
}
