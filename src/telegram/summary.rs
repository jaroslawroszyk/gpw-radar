use crate::calendar::fetch_strefa_inwestorow_calendar;
use crate::config::StockConfig;
use crate::metrics::ALERTS_SENT_COUNTER;
use crate::prices::get_stock_price;
use teloxide::prelude::*;
use teloxide::types::ParseMode;

pub async fn send_weekly_summary(
    bot: &Bot,
    http_client: &reqwest::Client,
    chat_id: ChatId,
    stocks: &[StockConfig],
    is_daily_close: bool,
) {
    let title = if is_daily_close {
        "🔔 <b>PODSUMOWANIE ZAMKNIĘCIA SESJI GPW</b>"
    } else {
        "📅 <b>PODSUMOWANIE PORTFELA I KALENDARZ GPW</b>"
    };

    let mut message = format!("{}\n\n📈 <b>Status Twoich Spółek:</b>\n", title);

    let mut best_stock: Option<(String, f64)> = None;
    let mut worst_stock: Option<(String, f64)> = None;

    for stock in stocks {
        if let Some(price) = get_stock_price(http_client, &stock.ticker).await {
            let trend = if price.change_pct >= 0.0 { "📈" } else { "📉" };
            message.push_str(&format!(
                "• <b>{} ({})</b>: {:.2} {} ({} {:+.2}%)\n",
                stock.name, stock.ticker, price.price, price.currency, trend, price.change_pct
            ));

            if best_stock.as_ref().is_none_or(|b| price.change_pct > b.1) {
                best_stock = Some((stock.name.clone(), price.change_pct));
            }
            if worst_stock.as_ref().is_none_or(|w| price.change_pct < w.1) {
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

    let strefa_calendar = fetch_strefa_inwestorow_calendar(http_client, stocks).await;

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
    ALERTS_SENT_COUNTER.inc();
}
