use serde::{Deserialize, Serialize};

pub use linear_regression_estimator::LinearRegressionEstimator;

pub mod linear_regression_estimator;

#[derive(Debug)]
pub struct DataPoint<Input, Output> {
    x: Input,
    y: Output,
}

trait Estimator<'de, Input, Output, Error>: Serialize + Deserialize<'de> {
    fn build(data: Vec<DataPoint<Input, Output>>) -> Result<Self, Error>;

    fn estimate(&self, x: Input) -> Output;
}
