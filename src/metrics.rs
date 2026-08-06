use prometheus::{HistogramOpts, HistogramVec, IntCounter, IntGauge};
use std::sync::LazyLock;

pub static NEWS_CHECKED_COUNTER: LazyLock<IntCounter> = LazyLock::new(|| {
    prometheus::register_int_counter!(
        "bot_news_checked_total",
        "Całkowita liczba przeanalizowanych nagłówków ESPI/newsów"
    )
    .unwrap()
});

pub static ALERTS_SENT_COUNTER: LazyLock<IntCounter> = LazyLock::new(|| {
    prometheus::register_int_counter!(
        "bot_alerts_sent_total",
        "Całkowita liczba wysłanych alertów na Telegram"
    )
    .unwrap()
});

pub static HTTP_ERRORS_COUNTER: LazyLock<IntCounter> = LazyLock::new(|| {
    prometheus::register_int_counter!("bot_http_errors_total", "Liczba błędów połączeń HTTP").unwrap()
});

pub static DB_STATUS_GAUGE: LazyLock<IntGauge> = LazyLock::new(|| {
    prometheus::register_int_gauge!("bot_db_status", "Status połączenia z bazą SQLite").unwrap()
});

pub static CYCLE_DURATION_HISTOGRAM: LazyLock<HistogramVec> = LazyLock::new(|| {
    prometheus::register_histogram_vec!(
        HistogramOpts::new(
            "bot_cycle_duration_seconds",
            "Czas trwania pełnego cyklu skanowania w sekundach"
        ),
        &["session_active"]
    )
    .unwrap()
});
