//! Text for the board. Its fonts (firmware/main/fonts, Inter subsets drawn at runtime)
//! have ASCII, Latin-1 (ä ö ü ß é …) and some punctuation (€ “ ” „ – — …), but no
//! other letters: those fold to their base letter (ł -> l, č -> c), anything else
//! (emoji, CJK) is dropped. Board buffers are bytes, so limits are bytes too.

/// Non-Latin-1 characters the board's fonts have (checked against the TTF cmap).
const FONT_EXTRA: &[char] = &[
    'ı', 'Œ', 'œ', '–', '—', '‘', '’', '‚', '“', '”', '„', '•', '…', '′', '″', '‹', '›', '€',
    '™', '−', '⁄',
];

/// Base letters of U+0100..U+017F (Latin Extended-A), in code point order.
const EXT_A_BASE: &str = concat!(
    "AaAaAa", "CcCcCcCc", "DdDd", "EeEeEeEeEe", "GgGgGgGg", "HhHh", "IiIiIiIiIi", "Ii", "Jj",
    "Kkk", "LlLlLlLlLl", "NnNnNnnNn", "OoOoOoOo", "RrRrRr", "SsSsSsSs", "TtTtTt",
    "UuUuUuUuUuUu", "Ww", "YyY", "ZzZzZzs",
);

/// Combining marks after a letter (text from macOS and some web pages) and the
/// Latin-1 letters they make: (base, mark, result).
const COMPOSE: &[(char, char, char)] = &[
    ('A', '\u{300}', 'À'), ('A', '\u{301}', 'Á'), ('A', '\u{302}', 'Â'), ('A', '\u{303}', 'Ã'), ('A', '\u{308}', 'Ä'), ('A', '\u{30A}', 'Å'),
    ('a', '\u{300}', 'à'), ('a', '\u{301}', 'á'), ('a', '\u{302}', 'â'), ('a', '\u{303}', 'ã'), ('a', '\u{308}', 'ä'), ('a', '\u{30A}', 'å'),
    ('C', '\u{327}', 'Ç'), ('c', '\u{327}', 'ç'),
    ('E', '\u{300}', 'È'), ('E', '\u{301}', 'É'), ('E', '\u{302}', 'Ê'), ('E', '\u{308}', 'Ë'),
    ('e', '\u{300}', 'è'), ('e', '\u{301}', 'é'), ('e', '\u{302}', 'ê'), ('e', '\u{308}', 'ë'),
    ('I', '\u{300}', 'Ì'), ('I', '\u{301}', 'Í'), ('I', '\u{302}', 'Î'), ('I', '\u{308}', 'Ï'),
    ('i', '\u{300}', 'ì'), ('i', '\u{301}', 'í'), ('i', '\u{302}', 'î'), ('i', '\u{308}', 'ï'),
    ('N', '\u{303}', 'Ñ'), ('n', '\u{303}', 'ñ'),
    ('O', '\u{300}', 'Ò'), ('O', '\u{301}', 'Ó'), ('O', '\u{302}', 'Ô'), ('O', '\u{303}', 'Õ'), ('O', '\u{308}', 'Ö'),
    ('o', '\u{300}', 'ò'), ('o', '\u{301}', 'ó'), ('o', '\u{302}', 'ô'), ('o', '\u{303}', 'õ'), ('o', '\u{308}', 'ö'),
    ('U', '\u{300}', 'Ù'), ('U', '\u{301}', 'Ú'), ('U', '\u{302}', 'Û'), ('U', '\u{308}', 'Ü'),
    ('u', '\u{300}', 'ù'), ('u', '\u{301}', 'ú'), ('u', '\u{302}', 'û'), ('u', '\u{308}', 'ü'),
    ('Y', '\u{301}', 'Ý'), ('y', '\u{301}', 'ý'), ('y', '\u{308}', 'ÿ'),
];

fn is_mark(c: char) -> bool {
    ('\u{300}'..='\u{36F}').contains(&c)
}

/// One character as the board can draw it: itself, a stand-in, or nothing.
fn board_char(c: char, out: &mut String) {
    let cp = c as u32;
    match c {
        // Invisible or unsupported spacing: soft hyphen, zero-width, BOM.
        '\u{AD}' | '\u{200B}'..='\u{200D}' | '\u{2060}' | '\u{FEFF}' => {}
        '\u{2010}'..='\u{2012}' | '\u{2015}' => out.push('-'),
        _ if c.is_ascii() || (0xA0..=0xFF).contains(&cp) || FONT_EXTRA.contains(&c) => out.push(c),
        _ if (0x100..=0x17F).contains(&cp) => {
            if let Some(b) = EXT_A_BASE.chars().nth((cp - 0x100) as usize) {
                out.push(b);
            }
        }
        _ => {}
    }
}

/// Text the board can draw: whitespace collapsed, at most `max_bytes` UTF-8 bytes,
/// never cut inside a character.
pub fn board_text(s: &str, max_bytes: usize) -> String {
    let mut drawn = String::with_capacity(s.len());
    let mut prev: Option<char> = None;
    for c in s.chars() {
        if is_mark(c) {
            // Join a combining mark with the letter before it when Latin-1 has the result.
            if let Some(&(_, _, joined)) = COMPOSE.iter().find(|(b, m, _)| Some(*b) == prev && *m == c) {
                drawn.pop();
                drawn.push(joined);
            }
            prev = None;
            continue;
        }
        board_char(c, &mut drawn);
        prev = Some(c);
    }
    let mut out = String::with_capacity(drawn.len().min(max_bytes));
    for word in drawn.split_whitespace() {
        let need = word.len() + usize::from(!out.is_empty());
        if out.len() + need > max_bytes {
            // Fit what still fits of this word, whole characters only.
            if !out.is_empty() && out.len() < max_bytes {
                out.push(' ');
            }
            for ch in word.chars() {
                if out.len() + ch.len_utf8() > max_bytes {
                    break;
                }
                out.push(ch);
            }
            return out.trim_end().to_string();
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_what_the_font_has() {
        assert_eq!(board_text("Besprechung mit Müller in Wien – Straße", 200), "Besprechung mit Müller in Wien – Straße");
        assert_eq!(board_text("„Ja“, sagt René… 5 €", 200), "„Ja“, sagt René… 5 €");
        assert_eq!(board_text("Œuvre ½ °C", 200), "Œuvre ½ °C");
    }

    #[test]
    fn folds_and_drops_the_rest() {
        assert_eq!(board_text("Łódź Žižkov Kraków", 200), "Lódz Zizkov Kraków");
        assert_eq!(board_text("Party 🎉 東京 now", 200), "Party now");
        assert_eq!(board_text("soft\u{AD}hy\u{200B}phen\u{2011}x", 200), "softhyphen-x");
    }

    #[test]
    fn composes_marks() {
        assert_eq!(board_text("Mu\u{308}ller Cafe\u{301}", 200), "Müller Café");
        assert_eq!(board_text("q\u{308}", 200), "q");
    }

    #[test]
    fn cuts_by_bytes_on_char_boundaries() {
        let s = board_text("ä ö ü ß", 7); // "ä ö ü" is 8 bytes
        assert!(s.len() <= 7 && s.is_char_boundary(s.len()));
        assert_eq!(s, "ä ö");
        assert_eq!(board_text("Müller", 2), "M");
        assert_eq!(board_text("  a   b  ", 10), "a b");
        assert_eq!(board_text("abcdef", 3), "abc");
        assert_eq!(board_text("", 5), "");
        for n in 0..30 {
            let t = board_text("Grüße aus Österreich, schöne Grüße", n);
            assert!(t.len() <= n);
        }
    }

    #[test]
    fn ext_a_table_covers_the_block() {
        assert_eq!(EXT_A_BASE.chars().count(), 0x80);
        let mut out = String::new();
        board_char('ő', &mut out);
        board_char('ł', &mut out);
        board_char('Ž', &mut out);
        assert_eq!(out, "olZ");
    }
}
