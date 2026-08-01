use rusqlite::Connection;
use serde::Deserialize;
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
pub struct AppConfig {
    pub stocks: Vec<StockConfig>,
}

#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase", description = "Dostępne komendy:")]
enum Command {
    #[command(description = "Wyświetla powitanie i menu pomocy.")]
    Start,
    #[command(description = "Wyświetla powitanie i menu pomocy.")]
    Help,
    #[command(description = "Wyświetla pełną listę śledzonych spółek.")]
    Portfel,
}

async fn answer_command(bot: Bot, msg: Message, cmd: Command, config: Arc<AppConfig>) -> ResponseResult<()> {
    match cmd {
        Command::Start | Command::Help => {
            bot.send_message(msg.chat.id, Command::descriptions().to_string())
                .await?;
        }
        Command::Portfel => {
            let mut text = String::from("Śledzone spółki:\n");
            for stock in &config.stocks {
                text.push_str(&format!("- {} ({})\n", stock.name, stock.ticker));
            }
            bot.send_message(msg.chat.id, text).await?;
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

    loop {
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
                            for stock in &config.stocks {
                                let mut combined_keywords = stock.keywords.clone();
                                combined_keywords.push(stock.name.clone());
                                combined_keywords.push(stock.ticker.replace(".WA", ""));

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

                                    let clean_title = sanitize_html(raw_title);
                                    let message = format!(
                                        "🚨 <b>[KOMUNIKAT - PORTFEL]</b>\n🏢 <b>{} ({})</b>\n📄 <b>Treść:</b> {}\n\n🔗 <a href=\"{}\">Otwórz raport ESPI</a>",
                                        stock.name, stock.ticker, clean_title, link
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

        tokio::time::sleep(Duration::from_secs(3 * 60)).await;
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
    .dependencies(dptree::deps![shared_config])
    .enable_ctrlc_handler()
    .build()
    .dispatch()
    .await;
}
