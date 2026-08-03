use chrono::{Datelike, Timelike, Utc, Weekday};
use chrono_tz::Europe::Warsaw;
use rusqlite::Connection;
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use teloxide::prelude::*;
use teloxide::types::ParseMode;
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
}

async fn answer_command(
    bot: Bot,
    msg: Message,
    cmd: Command,
    config: Arc<AppConfig>,
    db: Arc<Mutex<Connection>>,
) -> ResponseResult<()> {
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

                                    let clean_title = sanitize_html(raw_title);
                                    let message = format!(
                                        "🚨 <b>[KOMUNIKAT - PORTFEL]</b>\n🏢 <b>{} ({})</b>\n{}\n📄 <b>Treść:</b> {}\n\n🔗 <a href=\"{}\">Otwórz raport ESPI</a>",
                                        stock.name, stock.ticker, price_header, clean_title, link
                                    );

                                    let _ = bot
                                        .send_message(chat_id, message)
                                        .parse_mode(ParseMode::Html)
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

    Dispatcher::builder(
        bot,
        Update::filter_message()
            .filter_command::<Command>()
            .endpoint(answer_command),
    )
    .dependencies(dptree::deps![Arc::clone(&shared_config), Arc::clone(&db)])
    .enable_ctrlc_handler()
    .build()
    .dispatch()
    .await;
}
