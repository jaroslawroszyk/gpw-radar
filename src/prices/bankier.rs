use crate::models::PriceData;
use std::time::Duration;

pub async fn get_price_from_bankier(client: &reqwest::Client, ticker: &str) -> Option<PriceData> {
    let clean_symbol = ticker.replace(".WA", "").to_lowercase();
    let url = format!("https://www.bankier.pl/gielda/notowania/akcje/{}", clean_symbol);

    let res = client
        .get(&url)
        .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64)")
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .ok()?;

    let html_text = res.text().await.ok()?;
    let document = scraper::Html::parse_document(&html_text);

    let price_selector = scraper::Selector::parse(".profilHead .profilLast").unwrap();
    let change_selector = scraper::Selector::parse(".profilHead .change").unwrap();

    let price_str = document
        .select(&price_selector)
        .next()?
        .text()
        .collect::<String>();
    let clean_price_str = price_str.trim().replace(",", ".").replace(" ", "");
    let price: f64 = clean_price_str.parse().ok()?;

    let change_str = document
        .select(&change_selector)
        .next()
        .map(|e| e.text().collect::<String>())
        .unwrap_or_default();

    let change_pct = if change_str.contains('%') {
        let clean_pct = change_str
            .replace("%", "")
            .replace("(", "")
            .replace(")", "")
            .replace("+", "")
            .replace(",", ".")
            .trim()
            .to_string();
        clean_pct.parse::<f64>().unwrap_or(0.0)
    } else {
        0.0
    };

    let previous_close = if change_pct != -100.0 {
        price / (1.0 + (change_pct / 100.0))
    } else {
        price
    };

    Some(PriceData {
        price,
        previous_close,
        change_pct,
        currency: "PLN".to_string(),
        source: "Bankier.pl".to_string(),
    })
}
