use alloy::providers::RootProvider;
use alloy::transports::http::Http;
use reqwest::Client;

pub type ERC20Contract = erc20::IERC20::IERC20Instance<Http<Client>, RootProvider<Http<Client>>>;

pub mod erc20;
