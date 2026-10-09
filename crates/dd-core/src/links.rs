//! Links the hosted interface opens in a new window: pages of other sites, which belong in the driver's
//! browser. The app has one window without tabs or a back button, so a page of another site must never replace
//! the interface in it.

/// Whether a link the interface opens in a new window goes to the driver's browser: pages on the web only, so
/// a page can never have the app open a file or start a program this way.
pub fn opens_in_browser(url: &str) -> bool {
    let url = url.to_ascii_lowercase();
    url.starts_with("https://") || url.starts_with("http://")
}

/// Whether the app window may go to `url` itself: the platform (`platform`, its exact origin), Steam's sign-in
/// on `https://steamcommunity.com` (the Steam login runs in the window) and the bundled start page. Any other
/// page goes to the browser or nowhere: in a window without an address bar the driver cannot tell a copy of the
/// login page from the real one (security audit 2026-10-09, S8).
pub fn stays_in_app(url: &str, platform: &str) -> bool {
    let Ok(url) = url::Url::parse(url) else { return false };
    let start_page = matches!((url.scheme(), url.host_str()), ("tauri", Some("localhost")) | ("http" | "https", Some("tauri.localhost")));
    let steam = url.scheme() == "https" && url.host_str() == Some("steamcommunity.com") && url.port().is_none();
    let ours = url::Url::parse(platform).is_ok_and(|platform| url.origin() == platform.origin());
    start_page || steam || ours
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

    #[test]
    fn keeps_only_the_platform_and_steams_sign_in_in_the_window() {
        let platform = "https://digitaldrivers.club";
        for ok in [
            "https://digitaldrivers.club/",
            "https://digitaldrivers.club/evo/cars?x=1#y",
            "https://DigitalDrivers.club:443/auth/steam",
            "https://steamcommunity.com/openid/login?openid.mode=checkid_setup",
            // The bundled start page, as Tauri serves it on Windows and elsewhere.
            "http://tauri.localhost/index.html",
            "tauri://localhost/",
        ] {
            assert!(stays_in_app(ok, platform), "{ok}");
        }
        for away in [
            "http://digitaldrivers.club/",
            "https://race.digitaldrivers.club/",
            "https://digitaldrivers.club.evil.example/",
            "https://digitaldrivers.club@evil.example/",
            "https://digitaldrivers.club:8443/",
            "https://evil.example/https://digitaldrivers.club",
            "https://steamcommunity.com.evil.example/openid/login",
            "http://steamcommunity.com/openid/login",
            "https://store.steampowered.com/",
            "https://www.twitch.tv/digitaldrivers",
            "http://localhost:3000/",
            "javascript:alert(1)",
            "file:///C:/Windows/System32/cmd.exe",
            "not a url",
        ] {
            assert!(!stays_in_app(away, platform), "{away}");
        }
        // A development build against a platform on this machine (DD_PLATFORM_URL).
        assert!(stays_in_app("http://localhost:3000/evo", "http://localhost:3000"));
        assert!(!stays_in_app("http://localhost:3001/evo", "http://localhost:3000"));
    }
}
