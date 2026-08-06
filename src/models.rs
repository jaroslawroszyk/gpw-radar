#[derive(Debug, Clone)]
pub struct PriceData {
    pub price: f64,
    pub previous_close: f64,
    pub change_pct: f64,
    pub currency: String,
    pub source: String,
}

#[derive(Debug, Clone)]
pub struct CustomPriceAlert {
    pub ticker: String,
    pub target_price: f64,
    pub is_below: bool,
}

pub struct TechnicalIndicators {
    pub rsi_14: Option<f64>,
    pub sma_20: Option<f64>,
    pub trend_signal: &'static str,
}
