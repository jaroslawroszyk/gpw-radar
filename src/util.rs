pub fn normalize_ticker(raw: &str) -> String {
    let clean = raw.trim().to_uppercase();
    if clean.contains('.') {
        clean
    } else {
        format!("{}.WA", clean)
    }
}

pub fn sanitize_html(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

pub fn matches_keywords<'a>(title: &str, keywords: &'a [String]) -> Option<&'a str> {
    let title_lower = title.to_lowercase();
    keywords
        .iter()
        .find(|kw| title_lower.contains(&kw.to_lowercase()))
        .map(|kw| kw.as_str())
}
