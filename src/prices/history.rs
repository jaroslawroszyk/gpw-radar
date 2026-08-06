use std::time::Duration;
use tracing::{error, info};

pub async fn get_historical_prices(client: &reqwest::Client, ticker: &str) -> Vec<f64> {
    let clean_symbol = ticker.replace(".WA", "").to_lowercase();

    let stooq_ticker = format!("{}.va", clean_symbol);
    let stooq_csv_url = format!("https://stooq.pl/q/d/l/?s={}&i=d", stooq_ticker);

    if let Ok(res) = client
        .get(&stooq_csv_url)
        .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36")
        .header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8")
        .timeout(Duration::from_secs(6))
        .send()
        .await
        && let Ok(csv_text) = res.text().await {
            let mut prices = Vec::new();
            for line in csv_text.lines().skip(1) {
                let parts: Vec<&str> = line.split(',').collect();
                if let Some(close) = parts.get(4).and_then(|p| p.trim().parse::<f64>().ok())
                    && close > 0.0 {
                        prices.push(close);
                    }
                }

            if prices.len() >= 5 {
                info!(ticker = %ticker, count = prices.len(), "📊 Sparsowano ceny ze Stooq CSV (.va)");
                let take_count = prices.len().min(30);
                return prices[prices.len() - take_count..].to_vec();
            }
        }

    let br_url = format!(
        "https://www.biznesradar.pl/gielda/wykres-dane/{}",
        clean_symbol.to_uppercase()
    );
    if let Ok(res) = client
        .get(&br_url)
        .header(
            "User-Agent",
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)",
        )
        .timeout(Duration::from_secs(6))
        .send()
        .await
        && let Ok(json) = res.json::<serde_json::Value>().await
        && let Some(arr) = json.as_array()
    {
        let prices: Vec<f64> = arr
            .iter()
            .filter_map(|item| item.get(1).and_then(|v| v.as_f64()))
            .filter(|p| *p > 0.0)
            .collect();

        if prices.len() >= 5 {
            info!(ticker = %ticker, count = prices.len(), "📊 Sparsowano ceny z BiznesRadar");
            let take_count = prices.len().min(30);
            return prices[prices.len() - take_count..].to_vec();
        }
    }

    error!(ticker = %ticker, "❌ Brak danych ze wszystkich źródeł wykresowych");
    vec![]
}
