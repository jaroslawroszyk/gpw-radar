use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup};

pub fn build_espi_inline_keyboard(ticker: &str) -> InlineKeyboardMarkup {
    let clean_ticker = ticker.replace(".WA", "");
    let keyboard = vec![vec![
        InlineKeyboardButton::callback("📊 Wskaźniki", format!("stats_{}", clean_ticker)),
        InlineKeyboardButton::callback("📈 Wykres", format!("chart_{}", clean_ticker)),
        InlineKeyboardButton::callback("🔕 Mute/Unmute", format!("mute_{}", clean_ticker)),
    ]];
    InlineKeyboardMarkup::new(keyboard)
}
