//! Text as the Supertonic SDK (supertonic 1.3.1, MIT, Supertone Inc.) prepares it for the model: the long text is
//! cut into chunks of whole sentences (`utils.chunk_text`), and each chunk is normalised and wrapped in its
//! language tag (`core.UnicodeProcessor._preprocess_text`). Ported step by step, in the SDK's order.

use icu_normalizer::DecomposingNormalizerBorrowed;

/// The SDK's `DEFAULT_MAX_CHUNK_LENGTH` (characters) for every language but Korean.
pub const MAX_CHUNK: usize = 300;

fn is_emoji(c: char) -> bool {
    matches!(c as u32,
        0x1f600..=0x1f64f | 0x1f300..=0x1f5ff | 0x1f680..=0x1f6ff | 0x1f700..=0x1f77f | 0x1f780..=0x1f7ff
        | 0x1f800..=0x1f8ff | 0x1f900..=0x1f9ff | 0x1fa00..=0x1fa6f | 0x1fa70..=0x1faff | 0x2600..=0x26ff
        | 0x2700..=0x27bf | 0x1f1e6..=0x1f1ff)
}

/// `_SYMBOL_REPLACEMENTS` and `_remove_special_chars`, one character each.
fn symbol(c: char) -> Option<char> {
    Some(match c {
        '\u{2013}' | '\u{2011}' | '\u{2014}' => '-',
        '\u{00af}' | '_' | '[' | ']' | '|' | '/' | '#' | '→' | '←' => ' ',
        '\u{201c}' | '\u{201d}' => '"',
        '\u{2018}' | '\u{2019}' | '\u{00b4}' | '`' => '\'',
        '♥' | '☆' | '♡' | '©' | '\\' => return None,
        c => c,
    })
}

/// The characters that end a text without the SDK adding a full stop (`_ENDING_PUNCTUATION_PATTERN`).
const ENDINGS: &str = ".!?;:,'\")]}…。」』】〉》›»";

/// One chunk as the model reads it: `<de>Auto links.</de>`.
pub fn preprocess(text: &str, lang: &str) -> String {
    let nfkd = DecomposingNormalizerBorrowed::new_nfkd().normalize(text);
    let mut text: String = nfkd.chars().filter(|&c| !is_emoji(c)).filter_map(symbol).collect();
    for (from, to) in [("@", " at "), ("e.g.,", "for example, "), ("i.e.,", "that is, ")] {
        text = text.replace(from, to);
    }
    for (from, to) in [(" ,", ","), (" .", "."), (" !", "!"), (" ?", "?"), (" ;", ";"), (" :", ":"), (" '", "'")] {
        text = text.replace(from, to);
    }
    // A run of the same quote character becomes one.
    let mut quotes = String::with_capacity(text.len());
    for c in text.chars() {
        if !(matches!(c, '"' | '\'' | '`') && quotes.ends_with(c)) {
            quotes.push(c);
        }
    }
    let mut text = quotes.split_whitespace().collect::<Vec<_>>().join(" ");
    if !text.ends_with(|c| ENDINGS.contains(c)) {
        text.push('.');
    }
    format!("<{lang}>{text}</{lang}>")
}

/// The abbreviations after which a full stop does not end a sentence (`_COMMON_ABBREVIATIONS_PATTERN`).
const ABBREVIATIONS: [&str; 19] = [
    "Mr.", "Mrs.", "Ms.", "Dr.", "Prof.", "Sr.", "Jr.", "Ph.D.", "etc.", "e.g.", "i.e.", "vs.", "Inc.", "Ltd.", "Co.",
    "Corp.", "St.", "Ave.", "Blvd.",
];

/// Whether the SDK splits a sentence at whitespace that follows `before`.
fn ends_sentence(before: &str) -> bool {
    if !before.ends_with(['.', '!', '?']) || ABBREVIATIONS.iter().any(|a| before.ends_with(a)) {
        return false;
    }
    // `\b[A-Z]\.`: a single capital letter as an abbreviation ("J. Smith").
    let mut last = before.chars().rev();
    match (last.next(), last.next(), last.next()) {
        (Some('.'), Some(c), prev) if c.is_ascii_uppercase() => prev.is_some_and(|p| p.is_alphanumeric() || p == '_'),
        _ => true,
    }
}

fn sentences(paragraph: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut start, mut chars) = (0, paragraph.char_indices().peekable());
    while let Some((i, c)) = chars.next() {
        if c.is_whitespace() && ends_sentence(&paragraph[start..i]) {
            out.push(&paragraph[start..i]);
            while chars.next_if(|(_, c)| c.is_whitespace()).is_some() {}
            start = chars.peek().map_or(paragraph.len(), |(j, _)| *j);
        }
    }
    out.push(&paragraph[start..]);
    out
}

/// The text cut into chunks of whole sentences of at most `max` characters (a longer sentence stays whole):
/// paragraphs at blank lines, sentences after `.`, `!` or `?`.
pub fn chunks(text: &str, max: usize) -> Vec<String> {
    let mut out = Vec::new();
    for paragraph in split_paragraphs(text.trim()) {
        let mut current = String::new();
        for sentence in sentences(paragraph).into_iter().map(str::trim).filter(|s| !s.is_empty()) {
            if current.chars().count() + sentence.chars().count() < max {
                if !current.is_empty() {
                    current.push(' ');
                }
                current.push_str(sentence);
            } else {
                if !current.is_empty() {
                    out.push(std::mem::take(&mut current));
                }
                current = sentence.to_string();
            }
        }
        if !current.trim().is_empty() {
            out.push(current.trim().to_string());
        }
    }
    out
}

/// `re.split(r"\n\s*\n+", text)`, stripped, without empty paragraphs.
fn split_paragraphs(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\n' {
            // Whitespace after this newline that holds a second newline is a paragraph break.
            let rest = &text[i + 1..];
            let gap = rest.len() - rest.trim_start().len();
            if let Some(last_newline) = rest[..gap].rfind('\n') {
                out.push(&text[start..i]);
                // The regex ends at the last newline of the gap; the strip below drops the rest.
                i += 1 + last_newline + 1;
                start = i;
                continue;
            }
        }
        i += 1;
    }
    out.push(&text[start..]);
    out.into_iter().map(str::trim).filter(|p| !p.is_empty()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preprocess_matches_the_sdk() {
        // Expected values printed by supertonic 1.3.1's UnicodeProcessor._preprocess_text.
        assert_eq!(preprocess("Auto links", "de"), "<de>Auto links.</de>");
        assert_eq!(preprocess("  Car   left!  ", "en"), "<en>Car left!</en>");
        assert_eq!(preprocess("Jörn, Box diese Runde.", "de"), "<de>Jo\u{308}rn, Box diese Runde.</de>");
        assert_eq!(preprocess("Bremsen sehr heiß", "de"), "<de>Bremsen sehr heiß.</de>");
        assert_eq!(preprocess("Tyres — “hot” , aren’t they ?", "en"), "<en>Tyres - \"hot\", aren't they?</en>");
        assert_eq!(preprocess("a__b [x] 🚗 ©", "en"), "<en>a b x.</en>");
        assert_eq!(preprocess("Say \"\"hi\"\"", "en"), "<en>Say \"hi\"</en>");
        assert_eq!(preprocess("mail@dd e.g., this", "en"), "<en>mail at dd for example, this.</en>");
    }

    #[test]
    fn chunks_keep_short_text_whole_and_cut_long_text_at_sentences() {
        assert_eq!(chunks("Copy, Moment. Ich rechne kurz.", MAX_CHUNK), ["Copy, Moment. Ich rechne kurz."]);
        assert_eq!(chunks("One. Two! Three?", 10), ["One. Two!", "Three?"]);
        assert_eq!(chunks("Mr. Smith is here. J. Doe too.", 20), ["Mr. Smith is here.", "J. Doe too."]);
        assert_eq!(chunks("First.\n\n  Second.", MAX_CHUNK), ["First.", "Second."]);
        assert_eq!(chunks("Line one\nline two.", MAX_CHUNK), ["Line one\nline two."]);
        let long = "word ".repeat(80);
        assert_eq!(chunks(&long, MAX_CHUNK), [long.trim()], "a sentence longer than the limit stays whole");
        assert!(chunks("   ", MAX_CHUNK).is_empty());
    }
}
