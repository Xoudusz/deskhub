//! Valve KeyValues text (VDF/ACF) parser. Port of steam.py `parse_vdf`: tolerant of
//! quoted or bare tokens, nested braces, `//` comments and escapes; a repeated key keeps
//! its last value.

/// A parsed value: a string, or a block of keys in file order.
#[derive(Debug, Clone, PartialEq)]
pub enum Vdf {
    Str(String),
    Map(Vec<(String, Vdf)>),
}

impl Vdf {
    /// Case-insensitive child lookup. `None` for strings and missing keys.
    pub fn get(&self, key: &str) -> Option<&Vdf> {
        let want = key.to_lowercase();
        match self {
            Vdf::Map(entries) => entries
                .iter()
                .find(|(k, _)| k.to_lowercase() == want)
                .map(|(_, v)| v),
            Vdf::Str(_) => None,
        }
    }

    /// Case-insensitive descent (steam.py `_get`).
    pub fn path(&self, path: &[&str]) -> Option<&Vdf> {
        let mut cur = self;
        for key in path {
            cur = cur.get(key)?;
        }
        Some(cur)
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Vdf::Str(s) => Some(s),
            Vdf::Map(_) => None,
        }
    }

    /// Child entries; empty for strings.
    pub fn entries(&self) -> &[(String, Vdf)] {
        match self {
            Vdf::Map(entries) => entries,
            Vdf::Str(_) => &[],
        }
    }

    /// String child value, `""` when missing or a block.
    pub fn str_of(&self, key: &str) -> &str {
        self.get(key).and_then(Vdf::as_str).unwrap_or("")
    }

    /// Integer child value, `default` when missing or not a number (steam.py `_int`).
    pub fn int_of(&self, key: &str, default: i64) -> i64 {
        self.get(key)
            .and_then(Vdf::as_str)
            .and_then(|s| s.trim().parse::<i64>().ok())
            .unwrap_or(default)
    }
}

/// A token and whether it is a string (false only for bare `{` and `}`).
type Token = (String, bool);

fn tokenize(text: &str) -> Vec<Token> {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < n {
        let c = chars[i];
        if matches!(c, ' ' | '\t' | '\r' | '\n' | '\u{feff}') {
            i += 1;
        } else if c == '/' && chars.get(i + 1) == Some(&'/') {
            while i < n && chars[i] != '\n' {
                i += 1;
            }
        } else if c == '{' || c == '}' {
            tokens.push((c.to_string(), false));
            i += 1;
        } else if c == '"' {
            i += 1;
            let mut buf = String::new();
            while i < n && chars[i] != '"' {
                if chars[i] == '\\' && i + 1 < n {
                    buf.push(match chars[i + 1] {
                        'n' => '\n',
                        't' => '\t',
                        other => other,
                    });
                    i += 2;
                } else {
                    buf.push(chars[i]);
                    i += 1;
                }
            }
            i += 1;
            tokens.push((buf, true));
        } else {
            let start = i;
            while i < n && !matches!(chars[i], ' ' | '\t' | '\r' | '\n' | '{' | '}' | '"') {
                i += 1;
            }
            tokens.push((chars[start..i].iter().collect(), true));
        }
    }
    tokens
}

fn insert(out: &mut Vec<(String, Vdf)>, key: String, value: Vdf) {
    match out.iter_mut().find(|(k, _)| *k == key) {
        Some(slot) => slot.1 = value,
        None => out.push((key, value)),
    }
}

fn block(tokens: &[Token], pos: &mut usize) -> Vec<(String, Vdf)> {
    let mut out = Vec::new();
    while *pos < tokens.len() {
        let (tok, is_str) = &tokens[*pos];
        if tok == "}" && !is_str {
            *pos += 1;
            return out;
        }
        if tok == "{" && !is_str {
            *pos += 1; // stray brace
            continue;
        }
        let key = tok.clone();
        *pos += 1;
        if *pos >= tokens.len() {
            break;
        }
        let (next, next_str) = &tokens[*pos];
        if next == "{" && !next_str {
            *pos += 1;
            let inner = block(tokens, pos);
            insert(&mut out, key, Vdf::Map(inner));
        } else if next == "}" && !next_str {
            continue; // key without a value; the brace closes this block
        } else {
            insert(&mut out, key, Vdf::Str(next.clone()));
            *pos += 1;
        }
    }
    out
}

/// Parse VDF text into its top-level block.
pub fn parse_vdf(text: &str) -> Vdf {
    let tokens = tokenize(text);
    let mut pos = 0;
    Vdf::Map(block(&tokens, &mut pos))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_blocks_and_case_insensitive_lookup() {
        let v = parse_vdf(
            "\"AppState\"\n{\n\t\"appid\"\t\t\"570\"\n\t\"Name\" \"Dota 2\"\n\t\"UserConfig\" { \"language\" \"english\" }\n}\n",
        );
        let st = v.path(&["appstate"]).unwrap();
        assert_eq!(st.int_of("APPID", 0), 570);
        assert_eq!(st.str_of("name"), "Dota 2");
        assert_eq!(
            st.path(&["UserConfig", "language"]).unwrap().as_str(),
            Some("english")
        );
        assert!(st.path(&["nope", "x"]).is_none());
    }

    #[test]
    fn comments_bom_bare_tokens_and_escapes() {
        let v =
            parse_vdf("\u{feff}// header\nroot {\n a b // trailing\n \"p\" \"C:\\\\Games\\tX\"\n}");
        let r = v.get("root").unwrap();
        assert_eq!(r.str_of("a"), "b");
        assert_eq!(r.str_of("p"), "C:\\Games\tX");
    }

    #[test]
    fn repeated_key_keeps_last_value() {
        let v = parse_vdf("\"a\" \"1\" \"a\" \"2\"");
        assert_eq!(v.str_of("a"), "2");
        assert_eq!(v.entries().len(), 1);
    }

    #[test]
    fn stray_braces_and_valueless_key_tolerated() {
        let v = parse_vdf("{ \"x\" \"1\" \"o\" { \"k\" } \"y\" \"2\"");
        assert_eq!(v.str_of("x"), "1");
        assert_eq!(v.str_of("y"), "2");
        assert!(v.get("o").is_some());
        // A truncated file does not panic.
        let _ = parse_vdf("\"a\" { \"b\" \"c");
    }

    #[test]
    fn libraryfolders_shapes() {
        let v = parse_vdf(
            "\"libraryfolders\" { \"0\" { \"path\" \"C:\\\\Steam\" \"apps\" { \"570\" \"1\" } } \"1\" \"D:\\\\Lib\" }",
        );
        let lf = v.get("libraryfolders").unwrap();
        assert_eq!(lf.entries().len(), 2);
        assert_eq!(lf.entries()[0].1.str_of("path"), "C:\\Steam");
        assert_eq!(lf.entries()[1].1.as_str(), Some("D:\\Lib"));
    }

    #[test]
    fn int_of_defaults() {
        let v = parse_vdf("\"a\" \"x\" \"b\" \" 7 \"");
        assert_eq!(v.int_of("a", -1), -1);
        assert_eq!(v.int_of("b", -1), 7);
        assert_eq!(v.int_of("c", 3), 3);
    }
}
