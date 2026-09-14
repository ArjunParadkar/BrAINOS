//! A tiny flat-JSON scanner and writer for the wire protocol. Not a JSON
//! library: it reads string / number / bool values by key from one object,
//! extracts a nested object by key as a slice, and writes escaped strings.
//! That is all the body needs, and it costs no dependency on the board.

use alloc::string::String;

/// Find the raw value slice for `key` at the top level of `obj`.
fn raw_value<'a>(obj: &'a str, key: &str) -> Option<&'a str> {
    let b = obj.as_bytes();
    let mut i = 0;
    let mut depth = 0i32;
    let mut in_str = false;
    let mut esc = false;
    let mut key_start: Option<usize> = None;
    while i < b.len() {
        let c = b[i];
        if in_str {
            if esc {
                esc = false;
            } else if c == b'\\' {
                esc = true;
            } else if c == b'"' {
                in_str = false;
                if depth == 1 {
                    if let Some(ks) = key_start.take() {
                        let k = &obj[ks..i];
                        // is this string a key? next non-space must be ':'
                        let mut j = i + 1;
                        while j < b.len() && (b[j] == b' ' || b[j] == b'\n' || b[j] == b'\t' || b[j] == b'\r') {
                            j += 1;
                        }
                        if j < b.len() && b[j] == b':' {
                            j += 1;
                            while j < b.len() && (b[j] == b' ' || b[j] == b'\n' || b[j] == b'\t' || b[j] == b'\r') {
                                j += 1;
                            }
                            let vs = j;
                            let ve = value_end(b, vs);
                            if k == key {
                                return Some(&obj[vs..ve]);
                            }
                            i = ve;
                            continue;
                        }
                    }
                }
            }
            i += 1;
            continue;
        }
        match c {
            b'"' => {
                in_str = true;
                if depth == 1 {
                    key_start = Some(i + 1);
                }
            }
            b'{' | b'[' => depth += 1,
            b'}' | b']' => depth -= 1,
            _ => {}
        }
        i += 1;
    }
    None
}

/// End index (exclusive) of the value starting at `s`.
fn value_end(b: &[u8], s: usize) -> usize {
    if s >= b.len() {
        return s;
    }
    match b[s] {
        b'"' => {
            let mut i = s + 1;
            let mut esc = false;
            while i < b.len() {
                if esc {
                    esc = false;
                } else if b[i] == b'\\' {
                    esc = true;
                } else if b[i] == b'"' {
                    return i + 1;
                }
                i += 1;
            }
            b.len()
        }
        b'{' | b'[' => {
            let mut depth = 0i32;
            let mut in_str = false;
            let mut esc = false;
            let mut i = s;
            while i < b.len() {
                let c = b[i];
                if in_str {
                    if esc { esc = false } else if c == b'\\' { esc = true } else if c == b'"' { in_str = false }
                } else {
                    match c {
                        b'"' => in_str = true,
                        b'{' | b'[' => depth += 1,
                        b'}' | b']' => {
                            depth -= 1;
                            if depth == 0 {
                                return i + 1;
                            }
                        }
                        _ => {}
                    }
                }
                i += 1;
            }
            b.len()
        }
        _ => {
            let mut i = s;
            while i < b.len() && !matches!(b[i], b',' | b'}' | b']' | b' ' | b'\n' | b'\r' | b'\t') {
                i += 1;
            }
            i
        }
    }
}

/// Unescape a JSON string body (between the quotes). Handles \" \\ \/ \n \t \r and \uXXXX (BMP).
fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('"') => out.push('"'),
            Some('\\') => out.push('\\'),
            Some('/') => out.push('/'),
            Some('u') => {
                let mut v = 0u32;
                for _ in 0..4 {
                    v = v * 16 + it.next().and_then(|h| h.to_digit(16)).unwrap_or(0);
                }
                if let Some(ch) = char::from_u32(v) {
                    out.push(ch);
                }
            }
            Some(o) => {
                out.push('\\');
                out.push(o);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// String value for `key` (unescaped). None if absent or not a string.
pub fn get_str(obj: &str, key: &str) -> Option<String> {
    let v = raw_value(obj, key)?;
    if v.len() >= 2 && v.starts_with('"') && v.ends_with('"') {
        Some(unescape(&v[1..v.len() - 1]))
    } else {
        None
    }
}

/// Numeric value for `key`.
pub fn get_f32(obj: &str, key: &str) -> Option<f32> {
    raw_value(obj, key)?.parse::<f32>().ok()
}

pub fn get_u64(obj: &str, key: &str) -> Option<u64> {
    raw_value(obj, key)?.parse::<u64>().ok()
}

pub fn get_bool(obj: &str, key: &str) -> Option<bool> {
    match raw_value(obj, key)? {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

/// Nested object / array for `key`, as its raw slice.
pub fn get_obj<'a>(obj: &'a str, key: &str) -> Option<&'a str> {
    let v = raw_value(obj, key)?;
    if v.starts_with('{') || v.starts_with('[') {
        Some(v)
    } else {
        None
    }
}

/// Append a JSON string literal (with quotes) to `out`.
pub fn push_str(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                use core::fmt::Write;
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    const CMD: &str = r#"{"t":"cmd","id":7,"verb":"gait.walk","input":{"dir":"fwd","steps":3,"speed":0.5,"note":"a \"quoted\" {brace}"}}"#;

    #[test]
    fn reads_top_level_values() {
        assert_eq!(get_str(CMD, "t").as_deref(), Some("cmd"));
        assert_eq!(get_u64(CMD, "id"), Some(7));
        assert_eq!(get_str(CMD, "verb").as_deref(), Some("gait.walk"));
        assert_eq!(get_str(CMD, "dir"), None, "nested keys are not top level");
    }

    #[test]
    fn reads_nested_object_then_its_values() {
        let input = get_obj(CMD, "input").unwrap();
        assert!(input.starts_with('{') && input.ends_with('}'));
        assert_eq!(get_str(input, "dir").as_deref(), Some("fwd"));
        assert_eq!(get_f32(input, "steps"), Some(3.0));
        assert_eq!(get_f32(input, "speed"), Some(0.5));
        assert_eq!(get_str(input, "note").as_deref(), Some("a \"quoted\" {brace}"));
    }

    #[test]
    fn handles_bools_arrays_and_whitespace() {
        let s = r#"{ "ok" : true , "hands" : ["a","b"] , "n": -2.5 }"#;
        assert_eq!(get_bool(s, "ok"), Some(true));
        assert_eq!(get_obj(s, "hands"), Some(r#"["a","b"]"#));
        assert_eq!(get_f32(s, "n"), Some(-2.5));
        assert_eq!(get_str(s, "missing"), None);
    }

    #[test]
    fn writer_escapes() {
        let mut o = String::new();
        push_str(&mut o, "say \"hi\"\n\\");
        assert_eq!(o, r#""say \"hi\"\n\\""#);
        assert_eq!(get_str(&alloc::format!("{{\"x\":{o}}}"), "x").as_deref(), Some("say \"hi\"\n\\"));
    }
}
