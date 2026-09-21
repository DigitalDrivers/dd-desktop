//! Links the hosted interface hands to other programs: Content Manager's `acmanager://` links, and pages of
//! other sites, which belong in the driver's browser. The app has one window without tabs or a back button,
//! so a page of another site must never replace the interface in it.

use std::path::PathBuf;

/// The program behind a URL protocol, from the command Windows keeps for it under
/// `Software\Classes\<protocol>\shell\open\command`, e.g. `"C:\Games\Content Manager.exe" "%1"`.
pub fn protocol_handler_exe(command: &str) -> Option<PathBuf> {
    let command = command.trim();
    let exe = match command.strip_prefix('"') {
        Some(rest) => &rest[..rest.find('"')?],
        // Unquoted, the path may still contain spaces: it ends with the program's extension.
        None => match command.to_ascii_lowercase().find(".exe") {
            Some(end) => &command[..end + ".exe".len()],
            None => command.split_whitespace().next()?,
        },
    };
    (!exe.is_empty()).then(|| PathBuf::from(exe))
}

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
    fn finds_the_program_content_manager_registered() {
        // What Content Manager writes into the registry, taken from a real installation.
        assert_eq!(
            protocol_handler_exe(r#""C:\Program Files (x86)\Steam\steamapps\common\assettocorsa\Content Manager.exe" "%1""#),
            Some(PathBuf::from(r"C:\Program Files (x86)\Steam\steamapps\common\assettocorsa\Content Manager.exe"))
        );
        assert_eq!(protocol_handler_exe(r"C:\Games\AC\Content Manager.exe %1"), Some(PathBuf::from(r"C:\Games\AC\Content Manager.exe")));
        assert_eq!(protocol_handler_exe(r"C:\Tools\cm.EXE"), Some(PathBuf::from(r"C:\Tools\cm.EXE")));
    }

    #[test]
    fn finds_nothing_in_an_empty_or_broken_command() {
        assert_eq!(protocol_handler_exe(""), None);
        assert_eq!(protocol_handler_exe("   "), None);
        assert_eq!(protocol_handler_exe(r#""""#), None);
        assert_eq!(protocol_handler_exe(r#""C:\never closed"#), None);
    }

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
