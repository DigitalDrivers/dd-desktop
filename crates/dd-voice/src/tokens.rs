//! Whisper's token ids back to text: GPT-2's byte-level BPE, where every token is a string of stand-in characters,
//! one per byte (`bytes_to_unicode`: printable Latin-1 bytes stand for themselves, the other 68 bytes for U+0100
//! onwards). Decoding maps the characters back to bytes, joins the tokens' bytes and reads them as UTF-8, so a
//! character split across two tokens ("ö" is two bytes) comes back whole. The ids from `<|endoftext|>` on are the
//! special tokens (start of transcript, languages, tasks, timestamps); they carry no text.

use std::collections::HashMap;

/// The byte each stand-in character means (index: the character, U+0000..U+0143), after GPT-2's `bytes_to_unicode`.
fn byte_table() -> [Option<u8>; 0x144] {
    let printable = |b: u8| (0x21..=0x7e).contains(&b) || (0xa1..=0xac).contains(&b) || (0xae..=0xff).contains(&b);
    let mut table = [None; 0x144];
    // The bytes that are not printable, in order, stand for U+0100, U+0101, ..
    let mut next = 0x100;
    for b in 0..=255u8 {
        if printable(b) {
            table[b as usize] = Some(b);
        } else {
            table[next] = Some(b);
            next += 1;
        }
    }
    table
}

/// The text tokens' bytes, by id.
pub(crate) struct Vocab {
    pieces: Vec<Vec<u8>>,
}

impl Vocab {
    /// From `vocab.json` (token -> id). Ids below `first_special` are text; every one of them must be there.
    pub fn new(vocab: &HashMap<String, u32>, first_special: u32) -> Option<Self> {
        let table = byte_table();
        let byte_of = |c: char| table.get(c as usize).copied().flatten();
        let mut pieces: Vec<Option<Vec<u8>>> = vec![None; first_special as usize];
        for (token, &id) in vocab {
            if let Some(slot) = pieces.get_mut(id as usize) {
                *slot = Some(token.chars().map(byte_of).collect::<Option<Vec<u8>>>()?);
            }
        }
        Some(Vocab { pieces: pieces.into_iter().collect::<Option<Vec<_>>>()? })
    }

    /// The text of `ids`; special tokens are left out. Bytes that are not UTF-8 become U+FFFD.
    pub fn decode(&self, ids: &[u32]) -> String {
        let bytes: Vec<u8> = ids.iter().filter_map(|&id| self.pieces.get(id as usize)).flatten().copied().collect();
        String::from_utf8_lossy(&bytes).into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_byte_mapping_is_gpt2s() {
        let table = byte_table();
        let byte_of = |c: char| table.get(c as usize).copied().flatten();
        assert_eq!(byte_of('!'), Some(b'!'));
        assert_eq!(byte_of('Ġ'), Some(b' '), "U+0120 is the space");
        assert_eq!(byte_of('Ċ'), Some(b'\n'));
        assert_eq!(byte_of('Ā'), Some(0), "U+0100 is byte 0");
        assert_eq!(byte_of('Ã'), Some(0xc3));
        assert_eq!(byte_of('¶'), Some(0xb6));
        assert_eq!(byte_of('Ń'), Some(0xad), "the soft hyphen is the last stand-in");
        assert_eq!(byte_of(' '), None, "a real space never appears in a token");
        assert_eq!(byte_of('ń'), None);
        let mut all: Vec<u8> = table.iter().flatten().copied().collect();
        all.sort_unstable();
        assert_eq!(all, (0..=255).collect::<Vec<u8>>(), "every byte exactly once");
    }

    #[test]
    fn tokens_decode_to_text_and_special_ids_are_left_out() {
        // "Wie" "Ġviel" "ĠSprit" and "ö" (C3 B6) split into "Ã" and "¶" over two tokens, then ids 6 and 7 special.
        let vocab: HashMap<String, u32> =
            [("Wie", 0), ("Ġviel", 1), ("ĠSprit", 2), ("ĠH", 3), ("Ã", 4), ("¶he", 5)].into_iter().map(|(t, i)| (t.to_string(), i)).collect();
        let v = Vocab::new(&vocab, 6).unwrap();
        assert_eq!(v.decode(&[6, 0, 1, 2, 7]), "Wie viel Sprit");
        assert_eq!(v.decode(&[3, 4, 5]), " Höhe");
        assert_eq!(v.decode(&[3, 4]), " H\u{fffd}", "half a character");
        assert_eq!(v.decode(&[]), "");
        assert!(Vocab::new(&vocab, 7).is_none(), "a text id without a token");
        let bad: HashMap<String, u32> = [("a b".to_string(), 0)].into_iter().collect();
        assert!(Vocab::new(&bad, 1).is_none(), "a character that is no stand-in");
    }
}
