//! Links the hosted interface opens in a new window: pages of other sites, which belong in the driver's
//! browser. The app has one window without tabs or a back button, so a page of another site must never replace
//! the interface in it.

/// Whether a link the interface opens in a new window goes to the driver's browser: pages on the web only, so
/// a page can never have the app open a file or start a program this way.
pub fn opens_in_browser(url: &str) -> bool {
    let url = url.to_ascii_lowercase();
    url.starts_with("https://") || url.starts_with("http://")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sends_only_web_pages_to_the_browser() {
        assert!(opens_in_browser("https://www.twitch.tv/digitaldrivers"));
        assert!(opens_in_browser("HTTP://example.com/"));
        assert!(!opens_in_browser("acmanager://race/online/join?ip=race.digitaldrivers.club&httpPort=8110"));
        assert!(!opens_in_browser(r"file:///C:/Windows/System32/cmd.exe"));
        assert!(!opens_in_browser("javascript:alert(1)"));
        assert!(!opens_in_browser("ms-settings:"));
    }
}
