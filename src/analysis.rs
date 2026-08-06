use crate::models::TechnicalIndicators;
use tracing::error;

pub fn calculate_indicators(prices: &[f64]) -> TechnicalIndicators {
    if prices.len() < 14 {
        return TechnicalIndicators {
            rsi_14: None,
            sma_20: None,
            trend_signal: "NEUTRAL (Za mało danych)",
        };
    }

    let mut gains = 0.0;
    let mut losses = 0.0;

    for window in prices.windows(2).take(14) {
        let diff = window[1] - window[0];
        if diff >= 0.0 {
            gains += diff;
        } else {
            losses += diff.abs();
        }
    }

    let avg_gain = gains / 14.0;
    let avg_loss = losses / 14.0;

    let rsi = if avg_loss == 0.0 {
        100.0
    } else {
        let rs = avg_gain / avg_loss;
        100.0 - (100.0 / (1.0 + rs))
    };

    let sma_20 = if prices.len() >= 20 {
        let sum: f64 = prices.iter().rev().take(20).sum();
        Some(sum / 20.0)
    } else {
        None
    };

    let signal = if rsi > 70.0 {
        "🔴 OVERBOUGHT (RSI > 70)"
    } else if rsi < 30.0 {
        "🟢 OVERSOLD (RSI < 30)"
    } else {
        "⚪ NEUTRAL"
    };

    TechnicalIndicators {
        rsi_14: Some(rsi),
        sma_20,
        trend_signal: signal,
    }
}

pub fn generate_quickchart_url(ticker: &str, prices: &[f64]) -> String {
    let labels: Vec<String> = (1..=prices.len()).map(|i| format!("D{}", i)).collect();

    let chart_config = serde_json::json!({
        "type": "line",
        "data": {
            "labels": labels,
            "datasets": [
                {
                    "label": format!("Kurs {}", ticker),
                    "data": prices,
                    "borderColor": "#00a8ff",
                    "backgroundColor": "rgba(0, 168, 255, 0.05)",
                    "fill": true,
                    "borderWidth": 2,
                    "pointRadius": 1
                },
                {
                    "label": "SMA (5)",
                    "data": prices.iter().enumerate().map(|(idx, _)| {
                        if idx >= 4 {
                            let window = &prices[idx-4..=idx];
                            Some(window.iter().sum::<f64>() / 5.0)
                        } else {
                            None
                        }
                    }).collect::<Vec<Option<f64>>>(),
                    "borderColor": "#ff9f43",
                    "borderWidth": 1.5,
                    "fill": false,
                    "pointRadius": 0
                }
            ]
        },
        "options": {
            "title": { "display": true, "text": format!("Wykres z SMA - {}", ticker) },
            "legend": { "display": true }
        }
    });

    let chart_str = chart_config.to_string();
    format!(
        "https://quickchart.io/chart?c={}&bkg=white&w=600&h=350",
        urlencoding::encode(&chart_str)
    )
}

pub fn chart_caption(ticker: &str, indicators: &TechnicalIndicators) -> String {
    match indicators.rsi_14 {
        Some(rsi) => format!(
            "📈 Wykres z SMA dla {}\n📉 RSI (14): {:.2} | Sygnał: {}",
            ticker, rsi, indicators.trend_signal
        ),
        None => format!("📈 Wykres z SMA dla {}", ticker),
    }
}

/// QuickChart URL rośnie z liczbą punktów cenowych, więc zamiast `.unwrap()`
/// na parsowaniu (które potrafiło zabić cały task przy zbyt długim URL-u),
/// zwracamy `None` i logujemy błąd.
pub fn parse_chart_url(chart_url: &str) -> Option<reqwest::Url> {
    match chart_url.parse() {
        Ok(url) => Some(url),
        Err(e) => {
            error!(error = %e, "Nie udało się zparsować URL-a wykresu QuickChart");
            None
        }
    }
}
