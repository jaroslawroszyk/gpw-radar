use chrono::{Datelike, Timelike, Utc, Weekday};
use chrono_tz::Europe::Warsaw;
use rusqlite::Connection;
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use teloxide::prelude::*;
use teloxide::types::{
    CallbackQuery, InlineKeyboardButton, InlineKeyboardMarkup, InputFile, ParseMode,
};
use teloxide::utils::command::BotCommands;
use tracing::{error, info};
use tracing_subscriber::{EnvFilter, fmt};

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

#[derive(Debug, Clone)]
pub struct CustomPriceAlert {
    pub ticker: String,
    pub target_price: f64,
    pub is_below: bool,
}

#[derive(Debug, Clone)]
pub struct PriceData {
    pub price: f64,
    pub previous_close: f64,
    pub change_pct: f64,
    pub currency: String,
    pub source: String,
}

async fn get_price_from_yahoo(ticker: &str) -> Option<PriceData> {
    let url = format!(
        "https://query1.finance.yahoo.com/v8/finance/chart/{}",
        ticker
    );

    let client = reqwest::Client::new();
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

async fn get_price_from_bankier(ticker: &str) -> Option<PriceData> {
    let clean_symbol = ticker.replace(".WA", "").to_lowercase();
    let url = format!(
        "https://www.bankier.pl/gielda/notowania/akcje/{}",
        clean_symbol
    );

    let client = reqwest::Client::new();
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

async fn get_stock_price(ticker: &str) -> Option<PriceData> {
    if let Some(data) = get_price_from_yahoo(ticker).await {
        return Some(data);
    }
    tracing::warn!(ticker = %ticker, "Yahoo Finance nie odpowiedziało. Przełączanie na fallback Bankier.pl");
    get_price_from_bankier(ticker).await
}

pub struct TechnicalIndicators {
    pub rsi_14: Option<f64>,
    pub sma_20: Option<f64>,
    pub trend_signal: &'static str,
}

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

fn generate_quickchart_url_with_sma(ticker: &str, prices: &[f64]) -> String {
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

async fn get_historical_prices_yahoo(ticker: &str) -> Vec<f64> {
    let clean_symbol = ticker.replace(".WA", "").to_lowercase();

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(6))
        .build()
        .unwrap();

    let stooq_ticker = format!("{}.va", clean_symbol);
    let stooq_csv_url = format!("https://stooq.pl/q/d/l/?s={}&i=d", stooq_ticker);

    if let Ok(res) = client
        .get(&stooq_csv_url)
        .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36")
        .header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8")
        .send()
        .await
    {
        if let Ok(csv_text) = res.text().await {
            let mut prices = Vec::new();
            for line in csv_text.lines().skip(1) {
                let parts: Vec<&str> = line.split(',').collect();
                if parts.len() >= 5 {
                    if let Ok(close) = parts[4].trim().parse::<f64>() {
                        if close > 0.0 {
                            prices.push(close);
                        }
                    }
                }
            }

            if prices.len() >= 5 {
                info!(ticker = %ticker, count = prices.len(), "📊 Sparsowano ceny ze Stooq CSV (.va)");
                let take_count = prices.len().min(30);
                return prices[prices.len() - take_count..].to_vec();
            }
        }
    }

    let br_url = format!(
        "https://www.biznesradar.pl/gielda/wykres-dane/{}",
        clean_symbol.to_uppercase()
    );
    if let Ok(res) = client
        .get(&br_url)
        .header("User-Agent", "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)")
        .send()
        .await
    {
        if let Ok(json) = res.json::<serde_json::Value>().await {
            if let Some(arr) = json.as_array() {
                let mut prices = Vec::new();
                for item in arr {
                    if let Some(price) = item.get(1).and_then(|v| v.as_f64()) {
                        if price > 0.0 {
                            prices.push(price);
                        }
                    }
                }
                if prices.len() >= 5 {
                    info!(ticker = %ticker, count = prices.len(), "📊 Sparsowano ceny z BiznesRadar");
                    let take_count = prices.len().min(30);
                    return prices[prices.len() - take_count..].to_vec();
                }
            }
        }
    }

    error!(ticker = %ticker, "❌ Brak danych ze wszystkich źródeł wykresowych");
    vec![]
}

async fn generate_full_on_demand_analysis(
    http_client: &reqwest::Client,
    ticker: &str,
    recent_titles: &[String],
    price_info: Option<&PriceData>,
) -> String {
    let api_key = match std::env::var("GROQ_API_KEY") {
        Ok(k) if !k.is_empty() => k,
        _ => return "⚠️ Brak skonfigurowanego klucza GROQ_API_KEY w środowisku.".to_string(),
    };

    let price_str = match price_info {
        Some(p) => format!("{:.2} {} (zmiana: {:+.2}%)", p.price, p.currency, p.change_pct),
        None => "Brak danych cenowych".to_string(),
    };

    let titles_str = if recent_titles.is_empty() {
        "Brak ostatnich komunikatów w bazie".to_string()
    } else {
        recent_titles
            .iter()
            .map(|t| t.replace('"', "'").replace('\n', " "))
            .collect::<Vec<String>>()
            .join("\n• ")
    };

    let prompt = format!(
        "Jesteś Senior Analitykiem GPW. Przygotuj zwięzły raport analityczny dla spółki {}.\n\n\
        Aktualny kurs: {}\n\
        Ostatnie komunikaty spółki:\n• {}\n\n\
        Napisz raport z podziałem na:\n\
        1. 📊 Synteza sytuacji (Co się dzieje w spółce)\n\
        2. 🛡 Ocena ryzyka i potencjału (Fundamental Score 1-10)\n\
        3. 🎯 Rekomendacja dla inwestora wartościowego\n\n\
        Używaj prostego tekstu bez formatowania Markdown (bez gwiazdek i płotków).",
        ticker, price_str, titles_str
    );

    let url = "https://api.groq.com/openai/v1/chat/completions";
    let body = serde_json::json!({
        "model": "llama-3.3-70b-versatile",
        "messages": [{"role": "user", "content": prompt}],
        "temperature": 0.2,
        "max_tokens": 500
    });

    let res = match http_client
        .post(url)
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .json(&body)
        .timeout(Duration::from_secs(12))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => return format!("⚠️ Błąd sieciowy podczas łączenia z Groq: {}", e),
    };

    let status = res.status();
    let text_response = match res.text().await {
        Ok(t) => t,
        Err(e) => return format!("⚠️ Błąd odczytu odpowiedzi z Groq: {}", e),
    };

    if !status.is_success() {
        error!(status = %status, response = %text_response, "❌ Groq API zwróciło błąd HTTP");
        return format!("⚠️ API Groq zwróciło błąd HTTP {}: {}", status, text_response);
    }

    if let Ok(json_res) = serde_json::from_str::<serde_json::Value>(&text_response) {
        if let Some(content) = json_res["choices"][0]["message"]["content"].as_str() {
            return content.to_string();
        }
    }

    "⚠️ Nie udało się sparsować odpowiedzi z modelu AI.".to_string()
}

async fn summarize_espi_with_ai(http_client: &reqwest::Client, title: &str) -> Option<String> {
    let api_key = std::env::var("GROQ_API_KEY").ok()?;
    if api_key.is_empty() {
        return None;
    }

    let url = "https://api.groq.com/openai/v1/chat/completions";
    let prompt = format!(
        "Jesteś analitykiem giełdowym na GPW. Przeanalizuj poniższy komunikat ESPI i podaj odpowiedź w ścisłym formacie:\n\n\
        1. Linijka 1: Ocena wpływu w formacie: 🟢 [Impact: X/10 | BULLISH/BEARISH/NEUTRAL] (gdzie X to ocena 1-10)\n\
        2. Linijka 2: Streszczenie faktów (max 2 zdania po polsku, kwoty, daty, partnerzy).\n\n\
        Komunikat: {}",
        title
    );

    let body = serde_json::json!({
        "model": "llama-3.3-70b-versatile",
        "messages": [{"role": "user", "content": prompt}],
        "temperature": 0.1,
        "max_tokens": 140
    });

    let res = http_client
        .post(url)
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .json(&body)
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .ok()?;

    let json_res: serde_json::Value = res.json().await.ok()?;
    let ai_summary = json_res["choices"][0]["message"]["content"]
        .as_str()?
        .trim()
        .to_string();

    Some(ai_summary)
}

async fn evaluate_value_investing_with_ai(http_client: &reqwest::Client, title: &str) -> Option<String> {
    let api_key = std::env::var("GROQ_API_KEY").ok()?;
    if api_key.is_empty() {
        return None;
    }

    let url = "https://api.groq.com/openai/v1/chat/completions";
    let prompt = format!(
        "Jesteś inwestorem w wartość (Value Investor) kierującym się zasadami Benjamina Grahama i Warrena Buffetta. \
        Przeanalizuj komunikat ESPI z GPW pod kątem długoterminowej wartości, fosy rynkowej (Moat) i ryzyka. Podaj zwięzłą ocenę w MAX 2 zdaniach po polsku.\n\n\
        Komunikat: {}",
        title
    );

    let body = serde_json::json!({
        "model": "llama-3.3-70b-versatile",
        "messages": [{"role": "user", "content": prompt}],
        "temperature": 0.1,
        "max_tokens": 120
    });

    let res = http_client
        .post(url)
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .json(&body)
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .ok()?;

    let json_res: serde_json::Value = res.json().await.ok()?;
    let evaluation = json_res["choices"][0]["message"]["content"]
        .as_str()?
        .trim()
        .to_string();

    Some(evaluation)
}

fn parse_mar_insider_transaction(title: &str) -> Option<String> {
    let lower = title.to_lowercase();
    let is_mar = lower.contains("art. 19")
        || lower.contains("powiadomienie o transakcji")
        || lower.contains("zawiadomienie o transakcjach")
        || (lower.contains("nabycie") && lower.contains("akcji"))
        || (lower.contains("zbycie") && lower.contains("akcji"));

    if !is_mar {
        return None;
    }

    let action = if lower.contains("nabycie") || lower.contains("zakup") || lower.contains("kupno") {
        "🟢 <b>KUPNO (Nabycie)</b>"
    } else if lower.contains("zbycie") || lower.contains("sprzedaż") {
        "🔴 <b>SPRZEDAŻ (Zbycie)</b>"
    } else {
        "📊 <b>TRANSAKCJA INSIDERA</b>"
    };

    Some(format!(
        "⚠️ <b>[TRANSAKCJA INSIDERA / ART. 19 MAR]</b>\nTyp: {}\n📄 <b>Nagłówek:</b> {}",
        action, title
    ))
}

fn is_trading_hours() -> bool {
    let now = Utc::now().with_timezone(&Warsaw);
    let weekday = now.weekday();
    let hour = now.hour();
    let minute = now.minute();

    let is_weekend = weekday == Weekday::Sat || weekday == Weekday::Sun;

    !is_weekend
        && (hour > 8 || (hour == 8 && minute >= 30))
        && (hour < 17 || (hour == 17 && minute <= 30))
}

async fn fetch_strefa_inwestorow_calendar(stocks: &[StockConfig]) -> HashMap<String, String> {
    let mut calendar_map = HashMap::new();
    let url = "https://strefainwestorow.pl/dane/raporty/lista-publikacji-raportow-okresowych";

    let targets: Vec<String> = stocks
        .iter()
        .map(|s| s.ticker.replace(".WA", "").to_uppercase())
        .collect();

    let client = reqwest::Client::new();
    if let Ok(res) = client
        .get(url)
        .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64)")
        .send()
        .await
    {
        if let Ok(html_text) = res.text().await {
            let document = scraper::Html::parse_document(&html_text);
            let row_selector = scraper::Selector::parse("tr").unwrap();
            let cell_selector = scraper::Selector::parse("td, th").unwrap();

            for row in document.select(&row_selector) {
                let cells: Vec<String> = row
                    .select(&cell_selector)
                    .map(|c| c.text().collect::<String>().trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();

                if cells.len() >= 3 {
                    for cell in &cells {
                        let candidate = cell.to_uppercase();
                        if targets.iter().any(|t| t == &candidate) {
                            if let Some(date) = cells.iter().find(|c| c.contains("-") && c.len() >= 8) {
                                let report_type = cells
                                    .last()
                                    .cloned()
                                    .unwrap_or_else(|| "Raport okresowy".to_string());
                                calendar_map.insert(candidate, format!("{} ({})", date, report_type));
                            }
                        }
                    }
                }
            }
        }
    }

    calendar_map
}

async fn send_weekly_summary(bot: &Bot, chat_id: ChatId, stocks: &[StockConfig], is_daily_close: bool) {
    let title = if is_daily_close {
        "🔔 <b>PODSUMOWANIE ZAMKNIĘCIA SESJI GPW</b>"
    } else {
        "📅 <b>PODSUMOWANIE PORTFELA I KALENDARZ GPW</b>"
    };

    let mut message = format!("{}\n\n📈 <b>Status Twoich Spółek:</b>\n", title);

    let mut best_stock: Option<(String, f64)> = None;
    let mut worst_stock: Option<(String, f64)> = None;

    for stock in stocks {
        if let Some(price) = get_stock_price(&stock.ticker).await {
            let trend = if price.change_pct >= 0.0 { "📈" } else { "📉" };
            message.push_str(&format!(
                "• <b>{} ({})</b>: {:.2} {} ({} {:+.2}%)\n",
                stock.name, stock.ticker, price.price, price.currency, trend, price.change_pct
            ));

            if best_stock.as_ref().map_or(true, |b| price.change_pct > b.1) {
                best_stock = Some((stock.name.clone(), price.change_pct));
            }
            if worst_stock.as_ref().map_or(true, |w| price.change_pct < w.1) {
                worst_stock = Some((stock.name.clone(), price.change_pct));
            }
        } else {
            message.push_str(&format!("• <b>{}</b>: ⚠️ Błąd pobierania kursu\n", stock.name));
        }
    }

    if is_daily_close {
        if let (Some(best), Some(worst)) = (best_stock, worst_stock) {
            message.push_str(&format!(
                "\n🏆 <b>Lider dnia:</b> {} ({:+.2}%)\n🔻 <b>Maruder dnia:</b> {} ({:+.2}%)\n",
                best.0, best.1, worst.0, worst.1
            ));
        }
    }

    let strefa_calendar = fetch_strefa_inwestorow_calendar(stocks).await;

    message.push_str("\n🗓 <b>Nadchodzące Raporty Finansowe:</b>\n");
    for stock in stocks {
        let clean_ticker = stock.ticker.replace(".WA", "").to_uppercase();
        let report_info = strefa_calendar
            .get(&clean_ticker)
            .cloned()
            .unwrap_or_else(|| "Brak daty w kalendarzu".to_string());
        message.push_str(&format!("• <b>{}</b>: {}\n", stock.name, report_info));
    }

    let _ = bot.send_message(chat_id, message).parse_mode(ParseMode::Html).await;
}

#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase", description = "Dostępne komendy:")]
enum Command {
    #[command(description = "Wyświetla powitanie i menu pomocy.")]
    Start,
    #[command(description = "Wyświetla powitanie i menu pomocy.")]
    Help,
    #[command(description = "Pobiera natychmiastowy status cenowy portfela oraz kalendarz.")]
    Status,
    #[command(description = "Wyświetla pełną listę śledzonych spółek.")]
    Portfel,
    #[command(description = "Wyświetla tylko spółki dodane ręcznie.")]
    Lista,
    #[command(description = "Dodaje spółkę. Składnia: /dodaj TICKER")]
    Dodaj(String),
    #[command(description = "Usuwa spółkę z bazy. Składnia: /usun TICKER")]
    Usun(String),
    #[command(description = "Generuje syntetyczną analizę AI spółki. Składnia: /analiza TICKER")]
    Analiza(String),
    #[command(description = "Generuje wykres cenowy z wskaźnikami. Składnia: /wykres TICKER")]
    Wykres(String),
    #[command(
        description = "Ustawia spersonalizowany alert cenowy. Składnia: /alert TICKER < 120.50 lub /alert TICKER > 150"
    )]
    Alert(String),
    #[command(description = "Generuje raport portfela w pliku CSV.")]
    Eksport,
}

async fn answer_command(
    bot: Bot,
    msg: Message,
    cmd: Command,
    config: Arc<AppConfig>,
    db: Arc<Mutex<Connection>>,
) -> ResponseResult<()> {
    let http_client = reqwest::Client::new();

    match cmd {
        Command::Start | Command::Help => {
            bot.send_message(msg.chat.id, Command::descriptions().to_string())
                .await?;
        }
        Command::Status => {
            bot.send_message(msg.chat.id, "⏳ Pobieram aktualne kursy i kalendarz...")
                .await?;
            let tracked_stocks = {
                let conn = db.lock().unwrap();
                get_all_tracked_stocks(&config, &conn)
            };
            send_weekly_summary(&bot, msg.chat.id, &tracked_stocks, false).await;
        }
        Command::Portfel => {
            let tracked_stocks = {
                let conn = db.lock().unwrap();
                get_all_tracked_stocks(&config, &conn)
            };
            let mut text = String::from("Śledzone spółki:\n");
            for stock in &tracked_stocks {
                text.push_str(&format!("- {} ({})\n", stock.name, stock.ticker));
            }
            bot.send_message(msg.chat.id, text).await?;
        }
        Command::Lista => {
            let custom_stocks = {
                let conn = db.lock().unwrap();
                get_custom_stocks_from_db(&conn)
            };
            if custom_stocks.is_empty() {
                bot.send_message(msg.chat.id, "📌 Brak ręcznie dodanych spółek w bazie.")
                    .await?;
            } else {
                let mut text = String::from("Ręcznie dodane spółki:\n");
                for stock in custom_stocks {
                    text.push_str(&format!("- {} ({})\n", stock.name, stock.ticker));
                }
                bot.send_message(msg.chat.id, text).await?;
            }
        }
        Command::Dodaj(raw_ticker) => {
            let clean_ticker = raw_ticker.trim().to_uppercase();
            if clean_ticker.is_empty() {
                bot.send_message(msg.chat.id, "⚠️ Podaj ticker spółki! Przykład: /dodaj XTB.WA")
                    .await?;
                return Ok(());
            }
            let full_ticker = if !clean_ticker.contains('.') {
                format!("{}.WA", clean_ticker)
            } else {
                clean_ticker
            };

            if let Some(price) = get_stock_price(&full_ticker).await {
                let company_name = full_ticker.replace(".WA", "");
                let success = {
                    let conn = db.lock().unwrap();
                    add_custom_stock_to_db(&conn, &full_ticker, &company_name)
                };
                if success {
                    bot.send_message(
                        msg.chat.id,
                        format!(
                            "✅ Dodano {} ({}). Kurs: {:.2} {}",
                            company_name, full_ticker, price.price, price.currency
                        ),
                    )
                    .await?;
                } else {
                    bot.send_message(msg.chat.id, "⚠️ Błąd zapisu do bazy.").await?;
                }
            } else {
                bot.send_message(msg.chat.id, format!("❌ Nie znaleziono spółki {}.", full_ticker))
                    .await?;
            }
        }
        Command::Usun(raw_ticker) => {
            let clean_ticker = raw_ticker.trim().to_uppercase();
            let full_ticker = if !clean_ticker.contains('.') {
                format!("{}.WA", clean_ticker)
            } else {
                clean_ticker
            };
            let removed = {
                let conn = db.lock().unwrap();
                remove_custom_stock_from_db(&conn, &full_ticker)
            };
            if removed {
                bot.send_message(msg.chat.id, format!("🗑 Usunięto spółkę {} z bazy.", full_ticker))
                    .await?;
            } else {
                bot.send_message(
                    msg.chat.id,
                    format!("⚠️ Spółka {} nie znajdowała się w bazie custom_stocks.", full_ticker),
                )
                .await?;
            }
        }
        Command::Analiza(raw_ticker) => {
            let clean_ticker = raw_ticker.trim().to_uppercase();
            if clean_ticker.is_empty() {
                bot.send_message(msg.chat.id, "⚠️ Podaj ticker! Przykład: /analiza CDR")
                    .await?;
                return Ok(());
            }
            let full_ticker = if !clean_ticker.contains('.') {
                format!("{}.WA", clean_ticker)
            } else {
                clean_ticker
            };

            bot.send_message(
                msg.chat.id,
                format!("🧠 Generuję syntezę analityczną AI dla {}...", full_ticker),
            )
            .await?;

            let price_info = get_stock_price(&full_ticker).await;

            let recent_titles = {
                let conn = db.lock().unwrap();
                let mut titles = get_recent_titles_for_ticker(&conn, &full_ticker);
                if titles.is_empty() {
                    titles = get_recent_titles_for_ticker(&conn, &full_ticker.replace(".WA", ""));
                }
                titles
            };

            let analysis = generate_full_on_demand_analysis(
                &http_client,
                &full_ticker,
                &recent_titles,
                price_info.as_ref(),
            )
            .await;

            bot.send_message(msg.chat.id, analysis).await?;
        }
        Command::Wykres(raw_ticker) => {
            let clean_ticker = raw_ticker.trim().to_uppercase();
            if clean_ticker.is_empty() {
                bot.send_message(msg.chat.id, "⚠️ Podaj ticker! Przykład: /wykres CDR")
                    .await?;
                return Ok(());
            }
            let full_ticker = if !clean_ticker.contains('.') {
                format!("{}.WA", clean_ticker)
            } else {
                clean_ticker
            };

            bot.send_message(msg.chat.id, format!("📈 Generuję wykres dla {}...", full_ticker))
                .await?;

            let prices = get_historical_prices_yahoo(&full_ticker).await;
            if !prices.is_empty() {
                let chart_url = generate_quickchart_url_with_sma(&full_ticker, &prices);
                let tech_info = calculate_indicators(&prices);

                let caption = match tech_info.rsi_14 {
                    Some(rsi) => format!(
                        "📈 Wykres z SMA dla {}\n📉 RSI (14): {:.2} | Sygnał: {}",
                        full_ticker, rsi, tech_info.trend_signal
                    ),
                    None => format!("📈 Wykres z SMA dla {}", full_ticker),
                };

                let _ = bot
                    .send_photo(msg.chat.id, InputFile::url(chart_url.parse().unwrap()))
                    .caption(caption)
                    .await;
            } else {
                bot.send_message(
                    msg.chat.id,
                    format!("❌ Nie udało się pobrać danych historycznych dla {}", full_ticker),
                )
                .await?;
            }
        }
        Command::Alert(args) => {
            let parts: Vec<&str> = args.split_whitespace().collect();
            if parts.len() < 3 {
                bot.send_message(
                    msg.chat.id,
                    "⚠️ Składnia: /alert TICKER < PROG lub /alert TICKER > PROG\nPrzykład: /alert CDR < 115.50",
                )
                .await?;
                return Ok(());
            }

            let raw_ticker = parts[0].to_uppercase();
            let full_ticker = if !raw_ticker.contains('.') {
                format!("{}.WA", raw_ticker)
            } else {
                raw_ticker
            };

            let operator = parts[1];
            let target_price: f64 = match parts[2].replace(",", ".").parse() {
                Ok(val) => val,
                Err(_) => {
                    bot.send_message(msg.chat.id, "⚠️ Podano niepoprawną kwotę docelową!")
                        .await?;
                    return Ok(());
                }
            };

            let is_below = operator == "<";

            {
                let conn = db.lock().unwrap();
                add_user_price_alert(&conn, &full_ticker, target_price, is_below);
            }

            let direction = if is_below { "spadnie poniżej" } else { "wzrośnie powyżej" };
            bot.send_message(
                msg.chat.id,
                format!(
                    "✅ Ustawiono alert! Powiadomię Cię, gdy kurs {} {} {:.2} PLN.",
                    full_ticker, direction, target_price
                ),
            )
            .await?;
        }
        Command::Eksport => {
            let tracked_stocks = {
                let conn = db.lock().unwrap();
                get_all_tracked_stocks(&config, &conn)
            };
            let csv_bytes = {
                let conn = db.lock().unwrap();
                export_portfolio_csv(&tracked_stocks, &conn)
            };

            let document = InputFile::memory(csv_bytes).file_name("portfel_gpw.csv");
            bot.send_document(msg.chat.id, document)
                .caption("📄 Raport portfela w formacie CSV")
                .await?;
        }
    }
    Ok(())
}

fn init_db() -> Connection {
    let conn = Connection::open("bot_data.db").expect("Nie można otworzyć bazy SQLite");
    conn.execute(
        "CREATE TABLE IF NOT EXISTS seen_news (
            id TEXT PRIMARY KEY,
            title TEXT,
            ticker TEXT,
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP
        )",
        [],
    )
    .unwrap();

    conn.execute(
        "CREATE TABLE IF NOT EXISTS custom_stocks (
            ticker TEXT PRIMARY KEY,
            name TEXT NOT NULL
        )",
        [],
    )
    .unwrap();

    conn.execute(
        "CREATE TABLE IF NOT EXISTS user_price_alerts (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            ticker TEXT NOT NULL,
            target_price REAL NOT NULL,
            is_below INTEGER NOT NULL
        )",
        [],
    )
    .unwrap();

    conn.execute(
        "CREATE TABLE IF NOT EXISTS muted_stocks (
            ticker TEXT PRIMARY KEY
        )",
        [],
    )
    .unwrap();

    conn.execute(
        "CREATE TABLE IF NOT EXISTS price_alerts (
            ticker TEXT PRIMARY KEY,
            last_alerted_pct REAL,
            alerted_at DATETIME DEFAULT CURRENT_TIMESTAMP
        )",
        [],
    )
    .unwrap();

    conn
}

fn is_already_seen(conn: &Connection, news_id: &str) -> bool {
    let mut stmt = conn
        .prepare("SELECT COUNT(*) FROM seen_news WHERE id = ?1")
        .unwrap();
    let count: i64 = stmt.query_row([news_id], |row| row.get(0)).unwrap_or(0);
    count > 0
}

fn mark_as_seen(conn: &Connection, news_id: &str, title: &str, ticker: &str) {
    let _ = conn.execute(
        "INSERT OR IGNORE INTO seen_news (id, title, ticker) VALUES (?1, ?2, ?3)",
        [news_id, title, ticker],
    );
}

fn add_custom_stock_to_db(conn: &Connection, ticker: &str, name: &str) -> bool {
    conn.execute(
        "INSERT OR REPLACE INTO custom_stocks (ticker, name) VALUES (?1, ?2)",
        [ticker, name],
    )
    .is_ok()
}

fn remove_custom_stock_from_db(conn: &Connection, ticker: &str) -> bool {
    let count = conn
        .execute("DELETE FROM custom_stocks WHERE ticker = ?1", [ticker])
        .unwrap_or(0);
    count > 0
}

fn get_custom_stocks_from_db(conn: &Connection) -> Vec<StockConfig> {
    let mut stmt = match conn.prepare("SELECT ticker, name FROM custom_stocks") {
        Ok(s) => s,
        Err(_) => return vec![],
    };

    let stock_iter = stmt
        .query_map([], |row| {
            let ticker: String = row.get(0)?;
            let name: String = row.get(1)?;
            Ok(StockConfig {
                ticker,
                name,
                keywords: vec![],
            })
        })
        .unwrap();

    stock_iter.flatten().collect()
}

fn is_stock_muted(conn: &Connection, ticker: &str) -> bool {
    let mut stmt = conn
        .prepare("SELECT COUNT(*) FROM muted_stocks WHERE ticker = ?1")
        .unwrap();
    let count: i64 = stmt.query_row([ticker], |row| row.get(0)).unwrap_or(0);
    count > 0
}

fn toggle_mute_stock(conn: &Connection, ticker: &str) -> bool {
    if is_stock_muted(conn, ticker) {
        let _ = conn.execute("DELETE FROM muted_stocks WHERE ticker = ?1", [ticker]);
        false
    } else {
        let _ = conn.execute("INSERT OR REPLACE INTO muted_stocks (ticker) VALUES (?1)", [ticker]);
        true
    }
}

fn build_espi_inline_keyboard(ticker: &str) -> InlineKeyboardMarkup {
    let clean_ticker = ticker.replace(".WA", "");
    let keyboard = vec![vec![
        InlineKeyboardButton::callback("📊 Wskaźniki", format!("stats_{}", clean_ticker)),
        InlineKeyboardButton::callback("📈 Wykres", format!("chart_{}", clean_ticker)),
        InlineKeyboardButton::callback("🔕 Mute/Unmute", format!("mute_{}", clean_ticker)),
    ]];
    InlineKeyboardMarkup::new(keyboard)
}

async fn handle_callback_query(bot: Bot, q: CallbackQuery, db: Arc<Mutex<Connection>>) -> ResponseResult<()> {
    if let Some(data) = q.data {
        let chat_id = q.message.as_ref().map(|m| m.chat.id);

        if data.starts_with("stats_") {
            let ticker = format!("{}.WA", data.replace("stats_", ""));
            if let Some(price) = get_stock_price(&ticker).await {
                let prices = get_historical_prices_yahoo(&ticker).await;
                let tech_info = calculate_indicators(&prices);

                let rsi_str = match tech_info.rsi_14 {
                    Some(val) => format!("{:.2}", val),
                    None => "Brak danych".to_string(),
                };

                let text = format!(
                    "📊 <b>WSKAŹNIKI DLA {}</b>\n\n💵 Kurs: {:.2} {}\n📈 Zmiana: {:+.2}%\n\n📉 <b>RSI (14):</b> {}\n🎯 <b>Sygnał:</b> {}\n📊 Źródło danych: {}",
                    ticker, price.price, price.currency, price.change_pct, rsi_str, tech_info.trend_signal, price.source
                );
                if let Some(cid) = chat_id {
                    bot.send_message(cid, text).parse_mode(ParseMode::Html).await?;
                }
            }
        } else if data.starts_with("chart_") {
            let ticker = format!("{}.WA", data.replace("chart_", ""));
            let prices = get_historical_prices_yahoo(&ticker).await;
            if !prices.is_empty() {
                let chart_url = generate_quickchart_url_with_sma(&ticker, &prices);
                let tech_info = calculate_indicators(&prices);

                let caption = match tech_info.rsi_14 {
                    Some(rsi) => format!(
                        "📈 Wykres z SMA dla {}\n📉 RSI (14): {:.2} | Sygnał: {}",
                        ticker, rsi, tech_info.trend_signal
                    ),
                    None => format!("📈 Wykres z SMA dla {}", ticker),
                };

                if let Some(cid) = chat_id {
                    let _ = bot
                        .send_photo(cid, InputFile::url(chart_url.parse().unwrap()))
                        .caption(caption)
                        .await;
                }
            }
        } else if data.starts_with("mute_") {
            let ticker = format!("{}.WA", data.replace("mute_", ""));
            let is_now_muted = {
                let conn = db.lock().unwrap();
                toggle_mute_stock(&conn, &ticker)
            };

            let status_text = if is_now_muted {
                format!("🔕 Wyciszono powiadomienia ESPI dla spółki {}.", ticker)
            } else {
                format!("🔔 Przywrócono powiadomienia ESPI dla spółki {}.", ticker)
            };

            if let Some(cid) = chat_id {
                bot.send_message(cid, status_text).await?;
            }
        }
    }
    bot.answer_callback_query(q.id).await?;
    Ok(())
}

fn get_recent_titles_for_ticker(conn: &Connection, ticker: &str) -> Vec<String> {
    let mut stmt = match conn
        .prepare("SELECT title FROM seen_news WHERE ticker = ?1 ORDER BY created_at DESC LIMIT 5")
    {
        Ok(s) => s,
        Err(_) => return vec![],
    };

    let title_iter = stmt.query_map([ticker], |row| row.get(0)).unwrap();
    title_iter.flatten().collect()
}

fn add_user_price_alert(conn: &Connection, ticker: &str, target_price: f64, is_below: bool) {
    let _ = conn.execute(
        "INSERT INTO user_price_alerts (ticker, target_price, is_below) VALUES (?1, ?2, ?3)",
        (ticker, target_price, if is_below { 1 } else { 0 }),
    );
}

fn get_user_price_alerts(conn: &Connection) -> Vec<(i64, CustomPriceAlert)> {
    let mut stmt = match conn.prepare("SELECT id, ticker, target_price, is_below FROM user_price_alerts") {
        Ok(s) => s,
        Err(_) => return vec![],
    };

    let alert_iter = stmt
        .query_map([], |row| {
            let id: i64 = row.get(0)?;
            let ticker: String = row.get(1)?;
            let target_price: f64 = row.get(2)?;
            let is_below_int: i32 = row.get(3)?;
            Ok((
                id,
                CustomPriceAlert {
                    ticker,
                    target_price,
                    is_below: is_below_int == 1,
                },
            ))
        })
        .unwrap();

    alert_iter.flatten().collect()
}

fn remove_user_price_alert(conn: &Connection, id: i64) {
    let _ = conn.execute("DELETE FROM user_price_alerts WHERE id = ?1", [id]);
}

fn export_portfolio_csv(stocks: &[StockConfig], conn: &Connection) -> Vec<u8> {
    let mut csv_data = String::from("Ticker,Nazwa,Zbycie_Muted\n");
    for s in stocks {
        let is_muted = is_stock_muted(conn, &s.ticker);
        csv_data.push_str(&format!("{},\"{}\",{}\n", s.ticker, s.name, is_muted));
    }
    csv_data.into_bytes()
}

fn get_all_tracked_stocks(config: &AppConfig, conn: &Connection) -> Vec<StockConfig> {
    let mut all_stocks = config.stocks.clone();
    let custom_stocks = get_custom_stocks_from_db(conn);

    for cs in custom_stocks {
        if !all_stocks.iter().any(|s| s.ticker == cs.ticker) {
            all_stocks.push(cs);
        }
    }
    all_stocks
}

fn sanitize_html(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn matches_keywords<'a>(title: &'a str, keywords: &'a [String]) -> Option<&'a str> {
    let title_lower = title.to_lowercase();
    for kw in keywords {
        let kw_lower = kw.to_lowercase();
        if title_lower.contains(&kw_lower) {
            return Some(kw);
        }
    }
    None
}

async fn run_bot_loop(bot: Bot, chat_id: ChatId, config: Arc<AppConfig>, db: Arc<Mutex<Connection>>) {
    let espi_rss_url = "https://www.bankier.pl/rss/wiadomosci.xml";
    let ai_client = reqwest::Client::new();
    let mut weekly_summary_sent_this_week = false;
    let mut daily_close_sent_today = false;

    loop {
        let now = Utc::now().with_timezone(&Warsaw);
        let trading_active = is_trading_hours();

        let tracked_stocks = {
            let conn = db.lock().unwrap();
            get_all_tracked_stocks(&config, &conn)
        };

        if trading_active && now.hour() == 17 && now.minute() >= 5 && !daily_close_sent_today {
            send_weekly_summary(&bot, chat_id, &tracked_stocks, true).await;
            daily_close_sent_today = true;
        }
        if now.hour() != 17 {
            daily_close_sent_today = false;
        }

        if now.weekday() == Weekday::Sun && now.hour() == 18 {
            if !weekly_summary_sent_this_week {
                send_weekly_summary(&bot, chat_id, &tracked_stocks, false).await;
                weekly_summary_sent_this_week = true;
            }
        } else {
            weekly_summary_sent_this_week = false;
        }

        let active_user_alerts = {
            let conn = db.lock().unwrap();
            get_user_price_alerts(&conn)
        };

        for (alert_id, alert) in active_user_alerts {
            if let Some(price) = get_stock_price(&alert.ticker).await {
                let triggered = if alert.is_below {
                    price.price <= alert.target_price
                } else {
                    price.price >= alert.target_price
                };

                if triggered {
                    let dir_text = if alert.is_below { "spadł poniżej" } else { "wzrósł powyżej" };
                    let msg_text = format!(
                        "🎯 <b>[ALERT CENOWY OSIĄGNIĘTY]</b>\n🏢 <b>{}</b>\n💵 Aktualny kurs: <b>{:.2} {}</b> ({})\n🎯 Próg docelowy: {:.2} PLN",
                        alert.ticker, price.price, price.currency, dir_text, alert.target_price
                    );
                    let _ = bot.send_message(chat_id, msg_text).parse_mode(ParseMode::Html).await;

                    let conn = db.lock().unwrap();
                    remove_user_price_alert(&conn, alert_id);
                }
            }
        }

        for stock in &tracked_stocks {
            if let Some(price) = get_stock_price(&stock.ticker).await {
                if price.change_pct.abs() >= 3.0 {
                    let should_alert = {
                        let conn = db.lock().unwrap();
                        let mut stmt = conn
                            .prepare("SELECT last_alerted_pct FROM price_alerts WHERE ticker = ?1")
                            .unwrap();
                        let last_pct: Option<f64> = stmt.query_row([&stock.ticker], |r| r.get(0)).ok();

                        match last_pct {
                            Some(old) => (price.change_pct - old).abs() >= 2.0,
                            None => true,
                        }
                    };

                    if should_alert {
                        {
                            let conn = db.lock().unwrap();
                            let _ = conn.execute(
                                "INSERT OR REPLACE INTO price_alerts (ticker, last_alerted_pct) VALUES (?1, ?2)",
                                [&stock.ticker, &price.change_pct.to_string()],
                            );
                        }

                        let trend_icon = if price.change_pct >= 0.0 { "💥 📈" } else { "💥 📉" };
                        let spike_msg = format!(
                            "{} <b>[SKOK KURSU]</b>\n🏢 <b>{} ({})</b>\n💵 Kurs: <b>{:.2} {}</b> ({:+.2}%)\n📊 <i>Wykryto silny ruch cenowy! (Źródło: {})</i>",
                            trend_icon, stock.name, stock.ticker, price.price, price.currency, price.change_pct, price.source
                        );
                        let _ = bot
                            .send_message(chat_id, spike_msg)
                            .parse_mode(ParseMode::Html)
                            .await;
                    }
                }
            }
        }

        if let Ok(response) = reqwest::get(espi_rss_url).await {
            if let Ok(bytes) = response.bytes().await {
                if let Ok(feed) = feed_rs::parser::parse(&bytes[..]) {
                    for entry in feed.entries.iter().take(20) {
                        let link = entry.links.first().map(|l| l.href.as_str()).unwrap_or("");
                        let raw_title = entry
                            .title
                            .as_ref()
                            .map(|t| t.content.as_str())
                            .unwrap_or("Brak tytułu");

                        let is_seen = {
                            let conn = db.lock().unwrap();
                            is_already_seen(&conn, link)
                        };

                        if !link.is_empty() && !is_seen {
                            for stock in &tracked_stocks {
                                let mut combined_keywords = stock.keywords.clone();
                                combined_keywords.push(stock.name.clone());
                                combined_keywords.push(stock.ticker.replace(".WA", ""));
                                combined_keywords.extend(config.global_high_impact.clone());

                                if let Some(matched_kw) = matches_keywords(raw_title, &combined_keywords) {
                                    {
                                        let conn = db.lock().unwrap();
                                        mark_as_seen(&conn, link, raw_title, &stock.ticker);
                                    }

                                    let is_muted = {
                                        let conn = db.lock().unwrap();
                                        is_stock_muted(&conn, &stock.ticker)
                                    };
                                    if is_muted {
                                        info!(ticker = %stock.ticker, "Pominięto powiadomienie - spółka jest wyciszona.");
                                        break;
                                    }

                                    info!(
                                        ticker = %stock.ticker,
                                        keyword = %matched_kw,
                                        title = %raw_title,
                                        "Dopasowano komunikat ESPI dla spółki z portfela"
                                    );

                                    let price_header = match get_stock_price(&stock.ticker).await {
                                        Some(data) => format!(
                                            "<b>Kurs:</b> {:.2} {} ({:+.2}%, {})",
                                            data.price, data.currency, data.change_pct, data.source
                                        ),
                                        None => "<b>Kurs:</b> ⚠️ Brak danych".to_string(),
                                    };

                                    let mar_text = parse_mar_insider_transaction(raw_title)
                                        .map(|t| format!("\n{}\n", t))
                                        .unwrap_or_default();

                                    let ai_summary_text = match summarize_espi_with_ai(&ai_client, raw_title).await {
                                        Some(summary) => format!(
                                            "\n🤖 <b>Skrót & Sentyment AI:</b>\n<i>{}</i>",
                                            sanitize_html(&summary)
                                        ),
                                        None => String::new(),
                                    };

                                    let ai_buffett_text = match evaluate_value_investing_with_ai(&ai_client, raw_title).await {
                                        Some(eval) => format!(
                                            "\n🏛 <b>Ocena Buffetta & Grahama:</b> <i>{}</i>\n",
                                            sanitize_html(&eval)
                                        ),
                                        None => String::new(),
                                    };

                                    let clean_title = sanitize_html(raw_title);
                                    let message = format!(
                                        "🚨 <b>[KOMUNIKAT - PORTFEL]</b>\n🏢 <b>{} ({})</b>\n{}\n{}\n{}\n{}\n📄 <b>Treść:</b> {}\n\n🔗 <a href=\"{}\">Otwórz raport ESPI</a>",
                                        stock.name, stock.ticker, price_header, mar_text, ai_summary_text, ai_buffett_text, clean_title, link
                                    );

                                    let keyboard = build_espi_inline_keyboard(&stock.ticker);

                                    let _ = bot
                                        .send_message(chat_id, message)
                                        .parse_mode(ParseMode::Html)
                                        .reply_markup(keyboard)
                                        .await;
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        } else {
            error!("Błąd pobierania feedu RSS");
        }

        for source in &config.market_sources {
            if let Ok(response) = reqwest::get(&source.url).await {
                if let Ok(bytes) = response.bytes().await {
                    if let Ok(feed) = feed_rs::parser::parse(&bytes[..]) {
                        for entry in feed.entries.iter().take(3) {
                            let link = entry.links.first().map(|l| l.href.as_str()).unwrap_or("");
                            let raw_title = entry
                                .title
                                .as_ref()
                                .map(|t| t.content.as_str())
                                .unwrap_or("");

                            let is_seen = {
                                let conn = db.lock().unwrap();
                                is_already_seen(&conn, link)
                            };

                            if !link.is_empty() && !raw_title.is_empty() && !is_seen {
                                if let Some(kw) = matches_keywords(raw_title, &config.macro_keywords) {
                                    {
                                        let conn = db.lock().unwrap();
                                        mark_as_seen(&conn, link, raw_title, "MACRO");
                                    }

                                    info!(
                                        source = %source.name,
                                        keyword = %kw,
                                        title = %raw_title,
                                        "Dopasowano news makroekonomiczny"
                                    );

                                    let clean_title = sanitize_html(raw_title);
                                    let tag = if source.is_global {
                                        "🌍 <b>[NEWS GLOBALNY]</b>"
                                    } else {
                                        "🇵🇱 <b>[RYNEK POLSKI]</b>"
                                    };

                                    let message = format!(
                                        "{}\n📰 <b>{}</b>\n\n{}\n\n🔗 <a href=\"{}\">Czytaj artykuł</a>",
                                        tag, source.name, clean_title, link
                                    );

                                    let _ = bot
                                        .send_message(chat_id, message)
                                        .parse_mode(ParseMode::Html)
                                        .await;
                                }
                            }
                        }
                    }
                }
            }
        }

        let sleep_duration = if trading_active {
            Duration::from_secs(3 * 60)
        } else {
            Duration::from_secs(30 * 60)
        };
        tokio::time::sleep(sleep_duration).await;
    }
}

#[tokio::main]
async fn main() {
    let _ = dotenvy::dotenv_override();
    fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with_target(false)
        .init();

    let config_content = fs::read_to_string("config.toml").expect("Nie znaleziono config.toml");
    let config: AppConfig = toml::from_str(&config_content).expect("Niepoprawny format config.toml");
    let shared_config = Arc::new(config);

    let bot_token = std::env::var("TELOXIDE_TOKEN")
        .expect("Brak TELOXIDE_TOKEN w zmiennych środowiskowych!");
    let bot = Bot::new(bot_token);

    let chat_id_raw: i64 = std::env::var("CHAT_ID")
        .expect("Brak CHAT_ID w zmiennych środowiskowych!")
        .parse()
        .expect("CHAT_ID musi być liczbą!");
    let chat_id = ChatId(chat_id_raw);

    let db = Arc::new(Mutex::new(init_db()));

    info!("Bot uruchomiony, wczytano {} spółek", shared_config.stocks.len());

    let bg_bot = bot.clone();
    let bg_config = Arc::clone(&shared_config);
    let bg_db = Arc::clone(&db);
    tokio::spawn(async move {
        run_bot_loop(bg_bot, chat_id, bg_config, bg_db).await;
    });

    let callback_db = Arc::clone(&db);
    let handler = dptree::entry()
        .branch(
            Update::filter_message()
                .filter_command::<Command>()
                .endpoint(answer_command),
        )
        .branch(
            Update::filter_callback_query().endpoint(move |bot: Bot, q: CallbackQuery| {
                let db_ref = Arc::clone(&callback_db);
                async move { handle_callback_query(bot, q, db_ref).await }
            }),
        );

    Dispatcher::builder(bot, handler)
        .dependencies(dptree::deps![Arc::clone(&shared_config), Arc::clone(&db)])
        .enable_ctrlc_handler()
        .build()
        .dispatch()
        .await;
}
