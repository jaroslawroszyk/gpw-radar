use crate::models::PriceData;
use std::time::Duration;
use tracing::error;

const GROQ_URL: &str = "https://api.groq.com/openai/v1/chat/completions";
const MODEL: &str = "llama-3.3-70b-versatile";

async fn ask_groq(client: &reqwest::Client, prompt: String, max_tokens: u32, timeout_secs: u64) -> Option<String> {
    let api_key = std::env::var("GROQ_API_KEY").ok()?;
    if api_key.is_empty() {
        return None;
    }

    let body = serde_json::json!({
        "model": MODEL,
        "messages": [{"role": "user", "content": prompt}],
        "temperature": 0.1,
        "max_tokens": max_tokens
    });

    let res = client
        .post(GROQ_URL)
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .json(&body)
        .timeout(Duration::from_secs(timeout_secs))
        .send()
        .await
        .ok()?;

    let json_res: serde_json::Value = res.json().await.ok()?;
    let content = json_res["choices"][0]["message"]["content"].as_str()?.trim().to_string();
    Some(content)
}

pub async fn summarize_espi_with_ai(client: &reqwest::Client, title: &str) -> Option<String> {
    let prompt = format!(
        "Jesteś analitykiem giełdowym na GPW. Przeanalizuj poniższy komunikat ESPI i podaj odpowiedź w ścisłym formacie:\n\n\
        1. Linijka 1: Ocena wpływu w formacie: 🟢 [Impact: X/10 | BULLISH/BEARISH/NEUTRAL] (gdzie X to ocena 1-10)\n\
        2. Linijka 2: Streszczenie faktów (max 2 zdania po polsku, kwoty, daty, partnerzy).\n\n\
        Komunikat: {}",
        title
    );
    ask_groq(client, prompt, 140, 5).await
}

pub async fn evaluate_value_investing_with_ai(client: &reqwest::Client, title: &str) -> Option<String> {
    let prompt = format!(
        "Jesteś inwestorem w wartość (Value Investor) kierującym się zasadami Benjamina Grahama i Warrena Buffetta. \
        Przeanalizuj komunikat ESPI z GPW pod kątem długoterminowej wartości, fosy rynkowej (Moat) i ryzyka. Podaj zwięzłą ocenę w MAX 2 zdaniach po polsku.\n\n\
        Komunikat: {}",
        title
    );
    ask_groq(client, prompt, 120, 5).await
}

pub async fn generate_full_on_demand_analysis(
    http_client: &reqwest::Client,
    ticker: &str,
    recent_titles: &[String],
    price_info: Option<&PriceData>,
) -> String {
    let api_key = match std::env::var("GROQ_API_KEY") {
        Ok(k) if !k.is_empty() => k,
        _ => return "⚠️ Brak skonfigurowanego klucza GROQ_API_KEY w środowisku.".to_string(),
    };

    let price_str = match price_info {
        Some(p) => format!("{:.2} {} (zmiana: {:+.2}%)", p.price, p.currency, p.change_pct),
        None => "Brak danych cenowych".to_string(),
    };

    let titles_str = if recent_titles.is_empty() {
        "Brak ostatnich komunikatów w bazie".to_string()
    } else {
        recent_titles
            .iter()
            .map(|t| t.replace('"', "'").replace('\n', " "))
            .collect::<Vec<String>>()
            .join("\n• ")
    };

    let prompt = format!(
        "Jesteś Senior Analitykiem GPW. Przygotuj zwięzły raport analityczny dla spółki {}.\n\n\
        Aktualny kurs: {}\n\
        Ostatnie komunikaty spółki:\n• {}\n\n\
        Napisz raport z podziałem na:\n\
        1. 📊 Synteza sytuacji (Co się dzieje w spółce)\n\
        2. 🛡 Ocena ryzyka i potencjału (Fundamental Score 1-10)\n\
        3. 🎯 Rekomendacja dla inwestora wartościowego\n\n\
        Używaj prostego tekstu bez formatowania Markdown (bez gwiazdek i płotków).",
        ticker, price_str, titles_str
    );

    let body = serde_json::json!({
        "model": MODEL,
        "messages": [{"role": "user", "content": prompt}],
        "temperature": 0.2,
        "max_tokens": 500
    });

    let res = match http_client
        .post(GROQ_URL)
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .json(&body)
        .timeout(Duration::from_secs(12))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => return format!("⚠️ Błąd sieciowy podczas łączenia z Groq: {}", e),
    };

    let status = res.status();
    let text_response = match res.text().await {
        Ok(t) => t,
        Err(e) => return format!("⚠️ Błąd odczytu odpowiedzi z Groq: {}", e),
    };

    if !status.is_success() {
        error!(status = %status, response = %text_response, "❌ Groq API zwróciło błąd HTTP");
        return format!("⚠️ API Groq zwróciło błąd HTTP {}: {}", status, text_response);
    }

    if let Ok(json_res) = serde_json::from_str::<serde_json::Value>(&text_response) {
        if let Some(content) = json_res["choices"][0]["message"]["content"].as_str() {
            return content.to_string();
        }
    }

    "⚠️ Nie udało się sparsować odpowiedzi z modelu AI.".to_string()
}
