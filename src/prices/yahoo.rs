use crate::models::PriceData;
use std::time::Duration;

pub async fn get_price_from_yahoo(client: &reqwest::Client, ticker: &str) -> Option<PriceData> {
    let url = format!(
        "https://query1.finance.yahoo.com/v8/finance/chart/{}",
        ticker
    );

    let res = client
        .get(&url)
        .header("User-Agent", "Mozilla/5.0")
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .ok()?;

    let json: serde_json::Value = res.json().await.ok()?;
    let meta = &json["chart"]["result"][0]["meta"];

    let price = meta["regularMarketPrice"].as_f64()?;
    let previous_close = meta["chartPreviousClose"].as_f64().unwrap_or(price);
    let currency = meta["currency"].as_str().unwrap_or("PLN").to_string();

    let change_pct = if previous_close > 0.0 {
        ((price - previous_close) / previous_close) * 100.0
    } else {
        0.0
    };

    Some(PriceData {
        price,
        previous_close,
        change_pct,
        currency,
        source: "Yahoo Finance".to_string(),
    })
}
