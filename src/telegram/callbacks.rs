use crate::analysis::{calculate_indicators, chart_caption, generate_quickchart_url, parse_chart_url};
use crate::db::toggle_mute_stock;
use crate::prices::{get_historical_prices, get_stock_price};
use crate::util::normalize_ticker;
use rusqlite::Connection;
use std::sync::{Arc, Mutex};
use teloxide::prelude::*;
use teloxide::types::{CallbackQuery, InputFile, ParseMode};

pub async fn handle_callback_query(
    bot: Bot,
    q: CallbackQuery,
    http_client: reqwest::Client,
    db: Arc<Mutex<Connection>>,
) -> ResponseResult<()> {
    if let Some(data) = q.data {
        let chat_id = q.message.as_ref().map(|m| m.chat.id);

        if let Some(raw_ticker) = data.strip_prefix("stats_") {
            let ticker = normalize_ticker(raw_ticker);
            if let Some(price) = get_stock_price(&http_client, &ticker).await {
                let prices = get_historical_prices(&http_client, &ticker).await;
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
        } else if let Some(raw_ticker) = data.strip_prefix("chart_") {
            let ticker = normalize_ticker(raw_ticker);
            let prices = get_historical_prices(&http_client, &ticker).await;
            if !prices.is_empty() {
                let chart_url = generate_quickchart_url(&ticker, &prices);
                let tech_info = calculate_indicators(&prices);
                let caption = chart_caption(&ticker, &tech_info);

                if let (Some(cid), Some(url)) = (chat_id, parse_chart_url(&chart_url)) {
                    let _ = bot.send_photo(cid, InputFile::url(url)).caption(caption).await;
                }
            }
        } else if let Some(raw_ticker) = data.strip_prefix("mute_") {
            let ticker = normalize_ticker(raw_ticker);
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
