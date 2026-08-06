use crate::ai::generate_full_on_demand_analysis;
use crate::analysis::{
    calculate_indicators, chart_caption, generate_quickchart_url, parse_chart_url,
};
use crate::config::AppConfig;
use crate::db::{
    add_custom_stock_to_db, add_user_price_alert, export_portfolio_csv, get_all_tracked_stocks,
    get_custom_stocks_from_db, get_recent_titles_for_ticker, remove_custom_stock_from_db,
};
use crate::prices::{get_historical_prices, get_stock_price};
use crate::util::normalize_ticker;
use rusqlite::Connection;
use std::sync::{Arc, Mutex};
use teloxide::prelude::*;
use teloxide::types::InputFile;
use teloxide::utils::command::BotCommands;

use super::summary::send_weekly_summary;

#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase", description = "Dostępne komendy:")]
pub enum Command {
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

pub async fn answer_command(
    bot: Bot,
    msg: Message,
    cmd: Command,
    config: Arc<AppConfig>,
    db: Arc<Mutex<Connection>>,
    http_client: reqwest::Client,
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
            send_weekly_summary(&bot, &http_client, msg.chat.id, &tracked_stocks, false).await;
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
            if raw_ticker.trim().is_empty() {
                bot.send_message(
                    msg.chat.id,
                    "⚠️ Podaj ticker spółki! Przykład: /dodaj XTB.WA",
                )
                .await?;
                return Ok(());
            }
            let full_ticker = normalize_ticker(&raw_ticker);

            if let Some(price) = get_stock_price(&http_client, &full_ticker).await {
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
                    bot.send_message(msg.chat.id, "⚠️ Błąd zapisu do bazy.")
                        .await?;
                }
            } else {
                bot.send_message(
                    msg.chat.id,
                    format!("❌ Nie znaleziono spółki {}.", full_ticker),
                )
                .await?;
            }
        }
        Command::Usun(raw_ticker) => {
            let full_ticker = normalize_ticker(&raw_ticker);
            let removed = {
                let conn = db.lock().unwrap();
                remove_custom_stock_from_db(&conn, &full_ticker)
            };
            if removed {
                bot.send_message(
                    msg.chat.id,
                    format!("🗑 Usunięto spółkę {} z bazy.", full_ticker),
                )
                .await?;
            } else {
                bot.send_message(
                    msg.chat.id,
                    format!(
                        "⚠️ Spółka {} nie znajdowała się w bazie custom_stocks.",
                        full_ticker
                    ),
                )
                .await?;
            }
        }
        Command::Analiza(raw_ticker) => {
            if raw_ticker.trim().is_empty() {
                bot.send_message(msg.chat.id, "⚠️ Podaj ticker! Przykład: /analiza CDR")
                    .await?;
                return Ok(());
            }
            let full_ticker = normalize_ticker(&raw_ticker);

            bot.send_message(
                msg.chat.id,
                format!("🧠 Generuję syntezę analityczną AI dla {}...", full_ticker),
            )
            .await?;

            let price_info = get_stock_price(&http_client, &full_ticker).await;

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
            if raw_ticker.trim().is_empty() {
                bot.send_message(msg.chat.id, "⚠️ Podaj ticker! Przykład: /wykres CDR")
                    .await?;
                return Ok(());
            }
            let full_ticker = normalize_ticker(&raw_ticker);

            bot.send_message(
                msg.chat.id,
                format!("📈 Generuję wykres dla {}...", full_ticker),
            )
            .await?;

            let prices = get_historical_prices(&http_client, &full_ticker).await;
            if !prices.is_empty() {
                let chart_url = generate_quickchart_url(&full_ticker, &prices);
                let tech_info = calculate_indicators(&prices);
                let caption = chart_caption(&full_ticker, &tech_info);

                match parse_chart_url(&chart_url) {
                    Some(url) => {
                        let _ = bot
                            .send_photo(msg.chat.id, InputFile::url(url))
                            .caption(caption)
                            .await;
                    }
                    None => {
                        bot.send_message(msg.chat.id, "⚠️ Nie udało się wygenerować wykresu.")
                            .await?;
                    }
                }
            } else {
                bot.send_message(
                    msg.chat.id,
                    format!(
                        "❌ Nie udało się pobrać danych historycznych dla {}",
                        full_ticker
                    ),
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

            let full_ticker = normalize_ticker(parts[0]);
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

            let direction = if is_below {
                "spadnie poniżej"
            } else {
                "wzrośnie powyżej"
            };
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
