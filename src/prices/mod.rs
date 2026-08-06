mod bankier;
mod history;
mod yahoo;

pub use history::get_historical_prices;

use crate::models::PriceData;
use tracing::warn;

pub async fn get_stock_price(client: &reqwest::Client, ticker: &str) -> Option<PriceData> {
    if let Some(data) = yahoo::get_price_from_yahoo(client, ticker).await {
        return Some(data);
    }
    warn!(ticker = %ticker, "Yahoo Finance nie odpowiedziało. Przełączanie na fallback Bankier.pl");
    bankier::get_price_from_bankier(client, ticker).await
}
