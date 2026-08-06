use crate::config::{AppConfig, StockConfig};
use crate::metrics::DB_STATUS_GAUGE;
use crate::models::CustomPriceAlert;
use rusqlite::Connection;

pub fn init_db() -> Connection {
    let conn = Connection::open("bot_data.db").expect("Nie można otworzyć bazy SQLite");
    conn.execute(
        "CREATE TABLE IF NOT EXISTS seen_news (
            id TEXT PRIMARY KEY,
            title TEXT,
            ticker TEXT,
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP
        )",
        [],
    )
    .unwrap();

    conn.execute(
        "CREATE TABLE IF NOT EXISTS custom_stocks (
            ticker TEXT PRIMARY KEY,
            name TEXT NOT NULL
        )",
        [],
    )
    .unwrap();

    conn.execute(
        "CREATE TABLE IF NOT EXISTS user_price_alerts (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            ticker TEXT NOT NULL,
            target_price REAL NOT NULL,
            is_below INTEGER NOT NULL
        )",
        [],
    )
    .unwrap();

    conn.execute(
        "CREATE TABLE IF NOT EXISTS muted_stocks (
            ticker TEXT PRIMARY KEY
        )",
        [],
    )
    .unwrap();

    conn.execute(
        "CREATE TABLE IF NOT EXISTS price_alerts (
            ticker TEXT PRIMARY KEY,
            last_alerted_pct REAL,
            alerted_at DATETIME DEFAULT CURRENT_TIMESTAMP
        )",
        [],
    )
    .unwrap();

    DB_STATUS_GAUGE.set(1);
    conn
}

pub fn is_already_seen(conn: &Connection, news_id: &str) -> bool {
    let mut stmt = conn
        .prepare("SELECT COUNT(*) FROM seen_news WHERE id = ?1")
        .unwrap();
    let count: i64 = stmt.query_row([news_id], |row| row.get(0)).unwrap_or(0);
    count > 0
}

pub fn mark_as_seen(conn: &Connection, news_id: &str, title: &str, ticker: &str) {
    let _ = conn.execute(
        "INSERT OR IGNORE INTO seen_news (id, title, ticker) VALUES (?1, ?2, ?3)",
        [news_id, title, ticker],
    );
}

pub fn get_recent_titles_for_ticker(conn: &Connection, ticker: &str) -> Vec<String> {
    let mut stmt = match conn
        .prepare("SELECT title FROM seen_news WHERE ticker = ?1 ORDER BY created_at DESC LIMIT 5")
    {
        Ok(s) => s,
        Err(_) => return vec![],
    };

    let title_iter = stmt.query_map([ticker], |row| row.get(0)).unwrap();
    title_iter.flatten().collect()
}

pub fn add_custom_stock_to_db(conn: &Connection, ticker: &str, name: &str) -> bool {
    conn.execute(
        "INSERT OR REPLACE INTO custom_stocks (ticker, name) VALUES (?1, ?2)",
        [ticker, name],
    )
    .is_ok()
}

pub fn remove_custom_stock_from_db(conn: &Connection, ticker: &str) -> bool {
    let count = conn
        .execute("DELETE FROM custom_stocks WHERE ticker = ?1", [ticker])
        .unwrap_or(0);
    count > 0
}

pub fn get_custom_stocks_from_db(conn: &Connection) -> Vec<StockConfig> {
    let mut stmt = match conn.prepare("SELECT ticker, name FROM custom_stocks") {
        Ok(s) => s,
        Err(_) => return vec![],
    };

    let stock_iter = stmt
        .query_map([], |row| {
            let ticker: String = row.get(0)?;
            let name: String = row.get(1)?;
            Ok(StockConfig {
                ticker,
                name,
                keywords: vec![],
            })
        })
        .unwrap();

    stock_iter.flatten().collect()
}

pub fn get_all_tracked_stocks(config: &AppConfig, conn: &Connection) -> Vec<StockConfig> {
    let mut all_stocks = config.stocks.clone();
    let custom_stocks = get_custom_stocks_from_db(conn);

    for cs in custom_stocks {
        if !all_stocks.iter().any(|s| s.ticker == cs.ticker) {
            all_stocks.push(cs);
        }
    }
    all_stocks
}

pub fn is_stock_muted(conn: &Connection, ticker: &str) -> bool {
    let mut stmt = conn
        .prepare("SELECT COUNT(*) FROM muted_stocks WHERE ticker = ?1")
        .unwrap();
    let count: i64 = stmt.query_row([ticker], |row| row.get(0)).unwrap_or(0);
    count > 0
}

pub fn toggle_mute_stock(conn: &Connection, ticker: &str) -> bool {
    if is_stock_muted(conn, ticker) {
        let _ = conn.execute("DELETE FROM muted_stocks WHERE ticker = ?1", [ticker]);
        false
    } else {
        let _ = conn.execute("INSERT OR REPLACE INTO muted_stocks (ticker) VALUES (?1)", [ticker]);
        true
    }
}

pub fn should_alert_on_spike(conn: &Connection, ticker: &str, change_pct: f64) -> bool {
    let mut stmt = conn
        .prepare("SELECT last_alerted_pct FROM price_alerts WHERE ticker = ?1")
        .unwrap();
    let last_pct: Option<f64> = stmt.query_row([ticker], |r| r.get(0)).ok();

    match last_pct {
        Some(old) => (change_pct - old).abs() >= 2.0,
        None => true,
    }
}

pub fn record_spike_alert(conn: &Connection, ticker: &str, change_pct: f64) {
    let _ = conn.execute(
        "INSERT OR REPLACE INTO price_alerts (ticker, last_alerted_pct) VALUES (?1, ?2)",
        (ticker, change_pct),
    );
}

pub fn add_user_price_alert(conn: &Connection, ticker: &str, target_price: f64, is_below: bool) {
    let _ = conn.execute(
        "INSERT INTO user_price_alerts (ticker, target_price, is_below) VALUES (?1, ?2, ?3)",
        (ticker, target_price, if is_below { 1 } else { 0 }),
    );
}

pub fn get_user_price_alerts(conn: &Connection) -> Vec<(i64, CustomPriceAlert)> {
    let mut stmt = match conn.prepare("SELECT id, ticker, target_price, is_below FROM user_price_alerts") {
        Ok(s) => s,
        Err(_) => return vec![],
    };

    let alert_iter = stmt
        .query_map([], |row| {
            let id: i64 = row.get(0)?;
            let ticker: String = row.get(1)?;
            let target_price: f64 = row.get(2)?;
            let is_below_int: i32 = row.get(3)?;
            Ok((
                id,
                CustomPriceAlert {
                    ticker,
                    target_price,
                    is_below: is_below_int == 1,
                },
            ))
        })
        .unwrap();

    alert_iter.flatten().collect()
}

pub fn remove_user_price_alert(conn: &Connection, id: i64) {
    let _ = conn.execute("DELETE FROM user_price_alerts WHERE id = ?1", [id]);
}

pub fn export_portfolio_csv(stocks: &[StockConfig], conn: &Connection) -> Vec<u8> {
    let mut csv_data = String::from("Ticker,Nazwa,Zbycie_Muted\n");
    for s in stocks {
        let is_muted = is_stock_muted(conn, &s.ticker);
        csv_data.push_str(&format!("{},\"{}\",{}\n", s.ticker, s.name, is_muted));
    }
    csv_data.into_bytes()
}
