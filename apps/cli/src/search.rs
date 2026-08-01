//! Configurable omnibox search provider (DuckDuckGo default, env overrides).

/// Build a search URL for a free-text query.
pub fn search_url(query: &str) -> String {
    resolve_search_template().replace("{query}", &percent_encode(query))
}

fn resolve_search_template() -> String {
    if let Ok(custom) = std::env::var("TERMINAL_FENSTER_SEARCH_URL") {
        if custom.contains("{query}") {
            return custom;
        }
    }
    match std::env::var("TERMINAL_FENSTER_SEARCH")
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "google" => "https://www.google.com/search?q={query}".into(),
        "brave" => "https://search.brave.com/search?q={query}".into(),
        "bing" => "https://www.bing.com/search?q={query}".into(),
        "ecosia" => "https://www.ecosia.org/search?q={query}".into(),
        _ => "https://duckduckgo.com/?q={query}".into(),
    }
}

fn percent_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn default_is_duckduckgo() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::remove_var("TERMINAL_FENSTER_SEARCH");
        std::env::remove_var("TERMINAL_FENSTER_SEARCH_URL");
        let url = search_url("hello world");
        assert!(url.starts_with("https://duckduckgo.com/?q="));
        assert!(url.contains("hello%20world"));
    }

    #[test]
    fn custom_template_wins() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::remove_var("TERMINAL_FENSTER_SEARCH");
        std::env::remove_var("TERMINAL_FENSTER_SEARCH_URL");
        std::env::set_var(
            "TERMINAL_FENSTER_SEARCH_URL",
            "https://example.test/?q={query}",
        );
        let url = search_url("cats");
        assert_eq!(url, "https://example.test/?q=cats");
        std::env::remove_var("TERMINAL_FENSTER_SEARCH_URL");
    }
}
