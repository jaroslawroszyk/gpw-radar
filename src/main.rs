mod ai;
mod analysis;
mod calendar;
mod config;
mod db;
mod health;
mod insider;
mod metrics;
mod models;
mod prices;
mod scanner;
mod telegram;
mod time;
mod util;

use config::AppConfig;
use scanner::run_bot_loop;
use std::sync::{Arc, Mutex};
use teloxide::prelude::*;
use teloxide::utils::command::BotCommands;
use telegram::{Command, callbacks::handle_callback_query};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use tracing_subscriber::{EnvFilter, fmt};

#[tokio::main]
async fn main() {
    let _ = dotenvy::dotenv_override();
    fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with_target(false)
        .init();

    let shared_config = Arc::new(AppConfig::load("config.toml"));

    let bot_token = std::env::var("TELOXIDE_TOKEN").expect("Brak TELOXIDE_TOKEN w zmiennych środowiskowych!");
    let bot = Bot::new(bot_token);

    let chat_id_raw: i64 = std::env::var("CHAT_ID")
        .expect("Brak CHAT_ID w zmiennych środowiskowych!")
        .parse()
        .expect("CHAT_ID musi być liczbą!");
    let chat_id = ChatId(chat_id_raw);

    let db = Arc::new(Mutex::new(db::init_db()));
    let http_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) MarketBot/1.0")
        .build()
        .expect("Nie udało się zbudować klienta HTTP");

    info!("🤖 Bot uruchomiony, wczytano {} spółek", shared_config.stocks.len());

    let _ = bot.set_my_commands(Command::bot_commands()).await;

    let cancel_token = CancellationToken::new();

    let health_db = Arc::clone(&db);
    tokio::spawn(health::start_health_check_server(health_db));

    let bg_bot = bot.clone();
    let bg_config = Arc::clone(&shared_config);
    let bg_db = Arc::clone(&db);
    let bg_client = http_client.clone();
    let bg_token = cancel_token.clone();
    let loop_handle = tokio::spawn(async move {
        run_bot_loop(bg_bot, chat_id, bg_config, bg_db, bg_client, bg_token).await;
    });

    let callback_db = Arc::clone(&db);
    let callback_client = http_client.clone();
    let handler = dptree::entry()
        .branch(
            Update::filter_message()
                .filter_command::<Command>()
                .endpoint(telegram::answer_command),
        )
        .branch(
            Update::filter_callback_query().endpoint(move |bot: Bot, q: CallbackQuery| {
                let db_ref = Arc::clone(&callback_db);
                let client_ref = callback_client.clone();
                async move { handle_callback_query(bot, q, client_ref, db_ref).await }
            }),
        );

    let mut dispatcher = Dispatcher::builder(bot.clone(), handler)
        .dependencies(dptree::deps![
            Arc::clone(&shared_config),
            Arc::clone(&db),
            http_client.clone()
        ])
        .enable_ctrlc_handler()
        .build();

    let shutdown_bot = bot.clone();

    tokio::select! {
        _ = dispatcher.dispatch() => {},
        _ = tokio::signal::ctrl_c() => {
            warn!("🛑 Otrzymano sygnał SIGINT/SIGTERM. Rozpoczynanie Graceful Shutdown...");
            cancel_token.cancel();

            let _ = shutdown_bot
                .send_message(chat_id, "🛠 Bot przechodzi w stan konserwacji / restartu. Zamykanie połączeń...")
                .await;

            let _ = loop_handle.await;
            info!("👋 Wszystkie zadania zostały bezpiecznie zakończone. Aplikacja zatrzymana.");
        }
    }
}
