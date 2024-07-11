use alloy::providers::RootProvider;
use alloy::transports::http::Http;
use reqwest::Client;

use crate::blockchain::erc20::IERC20::IERC20Instance;

pub type ERC20Contract = IERC20Instance<Http<Client>, RootProvider<Http<Client>>>;

pub mod erc20;
