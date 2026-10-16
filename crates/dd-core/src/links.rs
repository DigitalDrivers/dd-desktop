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
/// on `https://steamcommunity.com` (the Steam login runs in the window) and the bundled start page, which a
/// development build (`npm run dev`) gets from Tauri's dev server at `dev_server` (its exact origin; the shell
/// passes it in debug builds only). Any other page goes to the browser or nowhere: in a window without an address
/// bar the driver cannot tell a copy of the login page from the real one (security audit 2026-10-09, S8).
pub fn stays_in_app(url: &str, platform: &str, dev_server: Option<&str>) -> bool {
    let Ok(url) = url::Url::parse(url) else { return false };
    let same_origin = |origin: &str| url::Url::parse(origin).is_ok_and(|origin| url.origin() == origin.origin());
    let start_page = matches!((url.scheme(), url.host_str()), ("tauri", Some("localhost")) | ("http" | "https", Some("tauri.localhost")));
    let steam = url.scheme() == "https" && url.host_str() == Some("steamcommunity.com") && url.port().is_none();
    start_page || steam || same_origin(platform) || dev_server.is_some_and(same_origin)
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
            assert!(stays_in_app(ok, platform, None), "{ok}");
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
            assert!(!stays_in_app(away, platform, None), "{away}");
        }
        // A development build against a platform on this machine (DD_PLATFORM_URL).
        assert!(stays_in_app("http://localhost:3000/evo", "http://localhost:3000", None));
        assert!(!stays_in_app("http://localhost:3001/evo", "http://localhost:3000", None));
    }

    #[test]
    fn a_development_build_keeps_the_start_page_of_tauri_s_dev_server() {
        let platform = "http://localhost:3000";
        // `npm run dev`: Tauri serves the bundled start page at 127.0.0.1:1430 (2026-10-16)
        let dev = Some("http://127.0.0.1:1430/");
        assert!(stays_in_app("http://127.0.0.1:1430/", platform, dev));
        assert!(stays_in_app("http://127.0.0.1:1430/index.html?x=1", platform, dev));
        for away in ["http://127.0.0.1:1431/", "https://127.0.0.1:1430/", "http://localhost:1430/", "http://192.168.0.2:1430/"] {
            assert!(!stays_in_app(away, platform, dev), "{away}");
        }
        // a release build passes no dev server: the start page comes only from the app itself
        assert!(!stays_in_app("http://127.0.0.1:1430/", platform, None));
        assert!(!stays_in_app("http://127.0.0.1:1430/", platform, Some("not a url")));
    }
}
