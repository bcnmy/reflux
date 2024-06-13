use serde::{Deserialize, Serialize};

use crate::estimator::{DataPoint, Estimator};

#[derive(Debug, Serialize, Deserialize)]
pub struct LinearRegressionEstimator {
    slope: f64,
    intercept: f64,
}

impl<'de> Estimator<'de, f64, f64, linreg::Error> for LinearRegressionEstimator {
    fn build(data: Vec<DataPoint<f64, f64>>) -> Result<Self, linreg::Error> {
        let (x, y): (Vec<f64>, Vec<f64>) =
            data.into_iter().map(|DataPoint { x, y }| (x, y)).unzip();
        let (slope, intercept) = linreg::linear_regression(&x, &y)?;
        Ok(Self { slope, intercept })
    }

    fn estimate(&self, x: f64) -> f64 {
        self.slope * x + self.intercept
    }
}

#[cfg(test)]
mod tests {
    use crate::estimator::{DataPoint, Estimator, LinearRegressionEstimator};

    #[test]
    fn test_should_recover_line_from_collinear_points() {
        let data: Vec<DataPoint<f64, f64>> = vec![
            DataPoint { x: 0.0, y: 0.0 },
            DataPoint { x: 1.0, y: 1.0 },
            DataPoint { x: 2.0, y: 2.0 },
        ];

        let estimator = LinearRegressionEstimator::build(data).unwrap();
        assert_eq!(estimator.estimate(3.0), 3.0);
        assert_eq!(estimator.estimate(4.0), 4.0);
        assert_eq!(estimator.estimate(5.0), 5.0);
    }
}
