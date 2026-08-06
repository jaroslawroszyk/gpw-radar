pub fn parse_mar_insider_transaction(title: &str) -> Option<String> {
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
