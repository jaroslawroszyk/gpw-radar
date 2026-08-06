use chrono::{Datelike, Timelike, Utc, Weekday};
use chrono_tz::Europe::Warsaw;

pub fn is_trading_hours() -> bool {
    let now = Utc::now().with_timezone(&Warsaw);
    let weekday = now.weekday();
    let hour = now.hour();
    let minute = now.minute();

    let is_weekend = weekday == Weekday::Sat || weekday == Weekday::Sun;

    !is_weekend
        && (hour > 8 || (hour == 8 && minute >= 30))
        && (hour < 17 || (hour == 17 && minute <= 30))
}
