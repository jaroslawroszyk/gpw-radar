use serde::Deserialize;
use std::fs;
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::utils::command::BotCommands;
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

    tracing::info!("Bot uruchomiony, wczytano {} spółek", shared_config.stocks.len());

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
