use serde::Deserialize;
use std::fs;

#[derive(Deserialize, Debug, Clone)]
pub struct StockConfig {
    pub ticker: String,
    pub name: String,
    pub keywords: Vec<String>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct MarketSourceConfig {
    pub name: String,
    pub url: String,
    pub is_global: bool,
}

#[derive(Deserialize, Debug, Clone)]
pub struct AppConfig {
    pub global_high_impact: Vec<String>,
    pub macro_keywords: Vec<String>,
    pub stocks: Vec<StockConfig>,
    pub market_sources: Vec<MarketSourceConfig>,
}

impl AppConfig {
    pub fn load(path: &str) -> Self {
        let content = fs::read_to_string(path).expect("Nie znaleziono config.toml");
        toml::from_str(&content).expect("Niepoprawny format config.toml")
    }
}
