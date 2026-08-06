use crate::ai::{evaluate_value_investing_with_ai, summarize_espi_with_ai};
use crate::config::AppConfig;
use crate::db::{
    get_all_tracked_stocks, get_user_price_alerts, is_already_seen, is_stock_muted, mark_as_seen,
    record_spike_alert, remove_user_price_alert, should_alert_on_spike,
};
use crate::insider::parse_mar_insider_transaction;
use crate::metrics::{
    ALERTS_SENT_COUNTER, CYCLE_DURATION_HISTOGRAM, HTTP_ERRORS_COUNTER, NEWS_CHECKED_COUNTER,
};
use crate::prices::get_stock_price;
use crate::telegram::keyboard::build_espi_inline_keyboard;
use crate::telegram::summary::send_weekly_summary;
use crate::time::is_trading_hours;
use crate::util::{matches_keywords, sanitize_html};
use chrono::{Datelike, Timelike, Utc, Weekday};
use chrono_tz::Europe::Warsaw;
use rusqlite::Connection;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use teloxide::prelude::*;
use teloxide::types::ParseMode;
use tokio_util::sync::CancellationToken;
use tracing::info;

const ESPI_RSS_URL: &str = "https://www.bankier.pl/rss/wiadomosci.xml";

pub async fn run_bot_loop(
    bot: Bot,
    chat_id: ChatId,
    config: Arc<AppConfig>,
    db: Arc<Mutex<Connection>>,
    http_client: reqwest::Client,
    cancel_token: CancellationToken,
) {
    let mut weekly_summary_sent_this_week = false;
    let mut daily_close_sent_today = false;
    let mut last_off_session_notification = Instant::now();

    loop {
        if cancel_token.is_cancelled() {
            info!("🛑 Wykryto sygnał zamknięcia w pętli bota.");
            break;
        }

        let cycle_start = Instant::now();
        let now = Utc::now().with_timezone(&Warsaw);
        let trading_active = is_trading_hours();
        let timer = CYCLE_DURATION_HISTOGRAM
            .with_label_values(&[&trading_active.to_string()])
            .start_timer();

        let tracked_stocks = {
            let conn = db.lock().unwrap();
            get_all_tracked_stocks(&config, &conn)
        };

        if !trading_active {
            if last_off_session_notification.elapsed() >= Duration::from_secs(8 * 3600) {
                let off_msg = "🌙 Rynek GPW jest obecnie zamknięty. Bot pracuje w trybie oszczędnym (co 30 min).";
                let _ = bot.send_message(chat_id, off_msg).await;
                last_off_session_notification = Instant::now();
            }
        } else {
            last_off_session_notification = Instant::now() - Duration::from_secs(8 * 3600);
        }

        if trading_active && now.hour() == 17 && now.minute() >= 5 && !daily_close_sent_today {
            send_weekly_summary(&bot, &http_client, chat_id, &tracked_stocks, true).await;
            daily_close_sent_today = true;
        }
        if now.hour() != 17 {
            daily_close_sent_today = false;
        }

        if now.weekday() == Weekday::Sun && now.hour() == 18 {
            if !weekly_summary_sent_this_week {
                send_weekly_summary(&bot, &http_client, chat_id, &tracked_stocks, false).await;
                weekly_summary_sent_this_week = true;
            }
        } else {
            weekly_summary_sent_this_week = false;
        }

        check_user_price_alerts(&bot, chat_id, &db, &http_client).await;
        check_price_spikes(&bot, chat_id, &db, &http_client, &tracked_stocks).await;

        if let Err(()) =
            scan_espi_feed(&bot, chat_id, &config, &db, &http_client, &tracked_stocks).await
        {
            HTTP_ERRORS_COUNTER.inc();
        }
        scan_macro_sources(&bot, chat_id, &config, &db, &http_client).await;

        timer.observe_duration();
        let cycle_duration = cycle_start.elapsed().as_millis();
        info!(
            duration_ms = cycle_duration,
            "📊 Podsumowanie cyklu skanowania"
        );

        let sleep_duration = if trading_active {
            Duration::from_secs(3 * 60)
        } else {
            Duration::from_secs(30 * 60)
        };
        tokio::select! {
            _ = tokio::time::sleep(sleep_duration) => {},
            _ = cancel_token.cancelled() => {
                info!("🛑 Wykryto anulowanie w trakcie uśpienia. Łagodne wychodzenie...");
                break;
            }
        }
    }
}

async fn check_user_price_alerts(
    bot: &Bot,
    chat_id: ChatId,
    db: &Arc<Mutex<Connection>>,
    http_client: &reqwest::Client,
) {
    let active_user_alerts = {
        let conn = db.lock().unwrap();
        get_user_price_alerts(&conn)
    };

    for (alert_id, alert) in active_user_alerts {
        let Some(price) = get_stock_price(http_client, &alert.ticker).await else {
            continue;
        };
        let triggered = if alert.is_below {
            price.price <= alert.target_price
        } else {
            price.price >= alert.target_price
        };

        if triggered {
            let dir_text = if alert.is_below {
                "spadł poniżej"
            } else {
                "wzrósł powyżej"
            };
            let msg_text = format!(
                "🎯 <b>[ALERT CENOWY OSIĄGNIĘTY]</b>\n🏢 <b>{}</b>\n💵 Aktualny kurs: <b>{:.2} {}</b> ({})\n🎯 Próg docelowy: {:.2} PLN",
                alert.ticker, price.price, price.currency, dir_text, alert.target_price
            );
            let _ = bot
                .send_message(chat_id, msg_text)
                .parse_mode(ParseMode::Html)
                .await;

            let conn = db.lock().unwrap();
            remove_user_price_alert(&conn, alert_id);
        }
    }
}

async fn check_price_spikes(
    bot: &Bot,
    chat_id: ChatId,
    db: &Arc<Mutex<Connection>>,
    http_client: &reqwest::Client,
    tracked_stocks: &[crate::config::StockConfig],
) {
    for stock in tracked_stocks {
        let Some(price) = get_stock_price(http_client, &stock.ticker).await else {
            continue;
        };
        if price.change_pct.abs() < 3.0 {
            continue;
        }

        let should_alert = {
            let conn = db.lock().unwrap();
            should_alert_on_spike(&conn, &stock.ticker, price.change_pct)
        };
        if !should_alert {
            continue;
        }

        {
            let conn = db.lock().unwrap();
            record_spike_alert(&conn, &stock.ticker, price.change_pct);
        }

        let trend_icon = if price.change_pct >= 0.0 {
            "💥 📈"
        } else {
            "💥 📉"
        };
        let spike_msg = format!(
            "{} <b>[SKOK KURSU]</b>\n🏢 <b>{} ({})</b>\n💵 Kurs: <b>{:.2} {}</b> ({:+.2}%)\n📊 <i>Wykryto silny ruch cenowy! (Źródło: {})</i>",
            trend_icon,
            stock.name,
            stock.ticker,
            price.price,
            price.currency,
            price.change_pct,
            price.source
        );
        let _ = bot
            .send_message(chat_id, spike_msg)
            .parse_mode(ParseMode::Html)
            .await;
        ALERTS_SENT_COUNTER.inc();
    }
}

async fn scan_espi_feed(
    bot: &Bot,
    chat_id: ChatId,
    config: &Arc<AppConfig>,
    db: &Arc<Mutex<Connection>>,
    http_client: &reqwest::Client,
    tracked_stocks: &[crate::config::StockConfig],
) -> Result<(), ()> {
    let response = http_client.get(ESPI_RSS_URL).send().await.map_err(|_| ())?;
    let bytes = response.bytes().await.map_err(|_| ())?;
    let Ok(feed) = feed_rs::parser::parse(&bytes[..]) else {
        return Ok(());
    };

    for entry in feed.entries.iter().take(20) {
        NEWS_CHECKED_COUNTER.inc();
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
        if link.is_empty() || is_seen {
            continue;
        }

        for stock in tracked_stocks {
            let mut combined_keywords = stock.keywords.clone();
            combined_keywords.push(stock.name.clone());
            combined_keywords.push(stock.ticker.replace(".WA", ""));
            combined_keywords.extend(config.global_high_impact.clone());

            let Some(matched_kw) = matches_keywords(raw_title, &combined_keywords) else {
                continue;
            };

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

            send_espi_alert(bot, chat_id, http_client, stock, raw_title, link).await;
            break;
        }
    }

    Ok(())
}

async fn send_espi_alert(
    bot: &Bot,
    chat_id: ChatId,
    http_client: &reqwest::Client,
    stock: &crate::config::StockConfig,
    raw_title: &str,
    link: &str,
) {
    let price_header = match get_stock_price(http_client, &stock.ticker).await {
        Some(data) => format!(
            "<b>Kurs:</b> {:.2} {} ({:+.2}%, {})",
            data.price, data.currency, data.change_pct, data.source
        ),
        None => "<b>Kurs:</b> ⚠️ Brak danych".to_string(),
    };

    let mar_text = parse_mar_insider_transaction(raw_title)
        .map(|t| format!("\n{}\n", t))
        .unwrap_or_default();

    let ai_summary_text = match summarize_espi_with_ai(http_client, raw_title).await {
        Some(summary) => format!(
            "\n🤖 <b>Skrót & Sentyment AI:</b>\n<i>{}</i>",
            sanitize_html(&summary)
        ),
        None => String::new(),
    };

    let ai_buffett_text = match evaluate_value_investing_with_ai(http_client, raw_title).await {
        Some(eval) => format!(
            "\n🏛 <b>Ocena Buffetta & Grahama:</b> <i>{}</i>\n",
            sanitize_html(&eval)
        ),
        None => String::new(),
    };

    let clean_title = sanitize_html(raw_title);
    let message = format!(
        "🚨 <b>[KOMUNIKAT - PORTFEL]</b>\n🏢 <b>{} ({})</b>\n{}\n{}\n{}\n{}\n📄 <b>Treść:</b> {}\n\n🔗 <a href=\"{}\">Otwórz raport ESPI</a>",
        stock.name,
        stock.ticker,
        price_header,
        mar_text,
        ai_summary_text,
        ai_buffett_text,
        clean_title,
        link
    );

    let keyboard = build_espi_inline_keyboard(&stock.ticker);

    let _ = bot
        .send_message(chat_id, message)
        .parse_mode(ParseMode::Html)
        .reply_markup(keyboard)
        .await;
    ALERTS_SENT_COUNTER.inc();
}

async fn scan_macro_sources(
    bot: &Bot,
    chat_id: ChatId,
    config: &Arc<AppConfig>,
    db: &Arc<Mutex<Connection>>,
    http_client: &reqwest::Client,
) {
    for source in &config.market_sources {
        let Ok(response) = http_client.get(&source.url).send().await else {
            HTTP_ERRORS_COUNTER.inc();
            continue;
        };
        let Ok(bytes) = response.bytes().await else {
            continue;
        };
        let Ok(feed) = feed_rs::parser::parse(&bytes[..]) else {
            continue;
        };

        for entry in feed.entries.iter().take(3) {
            NEWS_CHECKED_COUNTER.inc();
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
            if link.is_empty() || raw_title.is_empty() || is_seen {
                continue;
            }

            let Some(kw) = matches_keywords(raw_title, &config.macro_keywords) else {
                continue;
            };

            {
                let conn = db.lock().unwrap();
                mark_as_seen(&conn, link, raw_title, "MACRO");
            }

            info!(source = %source.name, keyword = %kw, title = %raw_title, "Dopasowano news makroekonomiczny");

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
            ALERTS_SENT_COUNTER.inc();
        }
    }
}
