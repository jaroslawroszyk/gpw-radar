use crate::config::StockConfig;
use std::collections::HashMap;

pub async fn fetch_strefa_inwestorow_calendar(
    client: &reqwest::Client,
    stocks: &[StockConfig],
) -> HashMap<String, String> {
    let mut calendar_map = HashMap::new();
    let url = "https://strefainwestorow.pl/dane/raporty/lista-publikacji-raportow-okresowych";

    let targets: Vec<String> = stocks
        .iter()
        .map(|s| s.ticker.replace(".WA", "").to_uppercase())
        .collect();

    let Ok(res) = client
        .get(url)
        .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64)")
        .send()
        .await
    else {
        return calendar_map;
    };

    let Ok(html_text) = res.text().await else {
        return calendar_map;
    };

    let document = scraper::Html::parse_document(&html_text);
    let row_selector = scraper::Selector::parse("tr").unwrap();
    let cell_selector = scraper::Selector::parse("td, th").unwrap();

    for row in document.select(&row_selector) {
        let cells: Vec<String> = row
            .select(&cell_selector)
            .map(|c| c.text().collect::<String>().trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        if cells.len() < 3 {
            continue;
        }

        for cell in &cells {
            let candidate = cell.to_uppercase();
            if !targets.iter().any(|t| t == &candidate) {
                continue;
            }
            if let Some(date) = cells.iter().find(|c| c.contains('-') && c.len() >= 8) {
                let report_type = cells
                    .last()
                    .cloned()
                    .unwrap_or_else(|| "Raport okresowy".to_string());
                calendar_map.insert(candidate, format!("{} ({})", date, report_type));
            }
        }
    }

    calendar_map
}
