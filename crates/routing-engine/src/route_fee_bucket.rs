use std::collections::HashMap;

pub struct RouteFeeBucket {
    pub cost_map: HashMap<String, (f64, f64, String)>, // (fee, quote, bridge_name)
}

impl RouteFeeBucket {
    pub fn new() -> Self {
        // Initialize with some dummy data
        let mut cost_map = HashMap::new();
        cost_map.insert("137_1_USDC_USDC".to_string(), (0.1, 100.0, "BridgeA".to_string()));
        cost_map.insert("42161_1_USDC_USDC".to_string(), (0.2, 50.0, "BridgeB".to_string()));
        Self { cost_map }
    }

    pub fn get_cost(
        &self,
        from_chain: u32,
        to_chain: u32,
        from_token: &str,
        to_token: &str,
    ) -> (f64, f64, String) {
        let key = format!("{}_{}_{}_{}", from_chain, to_chain, from_token, to_token);
        self.cost_map.get(&key).cloned().unwrap_or((1.0, 1.0, "DefaultBridge".to_string()))
    }
}
