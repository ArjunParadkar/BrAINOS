//! BFS  LEVEL    Section 8 Acquisition - adapters (ROS 2, MHS, the model boundary)  [crate brainos-adapters]
//! BFS  PURPOSE  Small hand-written JSON parser with hard limits on depth, string, array and input size, so untrusted device descriptions cannot crash or stall it
//! BFS  DEFINES  MAX_DEPTH, MAX_STRING, MAX_ITEMS, MAX_INPUT, JsonError, Json
//! BFS  USES     nothing outside core/alloc
//! BFS  USED BY  adapters/boundary, adapters/lib, adapters/mhs, adapters/tests/adapters_suite, proof/hostile
//!
//! A bounded JSON reader.
//!
//! Written rather than pulled in, for the reasons the whole core is
//! written rather than pulled in: it must work with no allocator beyond
//! `alloc`, it must not panic on hostile input, and it must be small
//! enough to understand completely.
//!
//! Everything is bounded: nesting depth, string length, array length,
//! and total input. A device description arriving over a wire is
//! untrusted input, and an unbounded parser on untrusted input is a
//! denial of service with extra steps.
//!
//! What it does not support: floating-point exponents beyond simple
//! forms, `\u` escapes outside the basic multilingual plane, and
//! duplicate keys (the last wins, and `is_duplicated` reports it, since
//! a schema with two different bounds for the same parameter is a schema
//! nobody should act on).

use alloc::string::String;
use alloc::vec::Vec;

pub const MAX_DEPTH: usize = 24;
pub const MAX_STRING: usize = 4_096;
pub const MAX_ITEMS: usize = 1_024;
pub const MAX_INPUT: usize = 1024 * 1024;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum JsonError {
    Empty,
    TooLarge,
    TooDeep,
    TooManyItems,
    StringTooLong,
    Unexpected(&'static str),
    Truncated,
    BadNumber,
    BadEscape,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

impl Json {
    pub fn parse(input: &str) -> Result<Json, JsonError> {
        if input.is_empty() {
            return Err(JsonError::Empty);
        }
        if input.len() > MAX_INPUT {
            return Err(JsonError::TooLarge);
        }
        let b = input.as_bytes();
        let mut p = Parser { b, i: 0, depth: 0 };
        p.skip_ws();
        let v = p.value()?;
        p.skip_ws();
        if p.i != b.len() {
            return Err(JsonError::Unexpected("trailing bytes"));
        }
        Ok(v)
    }

    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Object(fields) => fields.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Json::Number(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Json::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Json]> {
        match self {
            Json::Array(a) => Some(a),
            _ => None,
        }
    }

    pub fn as_object(&self) -> Option<&[(String, Json)]> {
        match self {
            Json::Object(o) => Some(o),
            _ => None,
        }
    }

    /// Does this object have the same key twice? A device description
    /// with two different bounds for one parameter is a description
    /// nobody should act on, and silently taking the last one is how a
    /// robot ends up moving to a limit the entity never saw.
    pub fn is_duplicated(&self) -> bool {
        match self {
            Json::Object(fields) => {
                for i in 1..fields.len() {
                    for j in 0..i {
                        if fields[i].0 == fields[j].0 {
                            return true;
                        }
                    }
                }
                fields.iter().any(|(_, v)| v.is_duplicated())
            }
            Json::Array(items) => items.iter().any(|v| v.is_duplicated()),
            _ => false,
        }
    }
}

struct Parser<'a> {
    b: &'a [u8],
    i: usize,
    depth: usize,
}

impl<'a> Parser<'a> {
    fn skip_ws(&mut self) {
        while self.i < self.b.len() && matches!(self.b[self.i], b' ' | b'\t' | b'\n' | b'\r') {
            self.i += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }

    fn eat(&mut self, c: u8) -> Result<(), JsonError> {
        if self.peek() == Some(c) {
            self.i += 1;
            Ok(())
        } else {
            Err(JsonError::Unexpected("expected a specific character"))
        }
    }

    fn literal(&mut self, word: &[u8]) -> Result<(), JsonError> {
        if self.b.len() >= self.i + word.len() && &self.b[self.i..self.i + word.len()] == word {
            self.i += word.len();
            Ok(())
        } else {
            Err(JsonError::Unexpected("expected a literal"))
        }
    }

    fn value(&mut self) -> Result<Json, JsonError> {
        if self.depth >= MAX_DEPTH {
            return Err(JsonError::TooDeep);
        }
        match self.peek().ok_or(JsonError::Truncated)? {
            b'n' => {
                self.literal(b"null")?;
                Ok(Json::Null)
            }
            b't' => {
                self.literal(b"true")?;
                Ok(Json::Bool(true))
            }
            b'f' => {
                self.literal(b"false")?;
                Ok(Json::Bool(false))
            }
            b'"' => self.string().map(Json::String),
            b'[' => self.array(),
            b'{' => self.object(),
            b'-' | b'0'..=b'9' => self.number(),
            _ => Err(JsonError::Unexpected("not a json value")),
        }
    }

    fn string(&mut self) -> Result<String, JsonError> {
        self.eat(b'"')?;
        let mut out = String::new();
        loop {
            let c = self.peek().ok_or(JsonError::Truncated)?;
            self.i += 1;
            match c {
                b'"' => return Ok(out),
                b'\\' => {
                    let e = self.peek().ok_or(JsonError::Truncated)?;
                    self.i += 1;
                    out.push(match e {
                        b'"' => '"',
                        b'\\' => '\\',
                        b'/' => '/',
                        b'b' => '\u{0008}',
                        b'f' => '\u{000C}',
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        b'u' => {
                            if self.i + 4 > self.b.len() {
                                return Err(JsonError::Truncated);
                            }
                            let mut code: u32 = 0;
                            for k in 0..4 {
                                let d = self.b[self.i + k];
                                let v = match d {
                                    b'0'..=b'9' => d - b'0',
                                    b'a'..=b'f' => d - b'a' + 10,
                                    b'A'..=b'F' => d - b'A' + 10,
                                    _ => return Err(JsonError::BadEscape),
                                };
                                code = code * 16 + v as u32;
                            }
                            self.i += 4;
                            // surrogates are refused rather than replaced:
                            // a description containing a lone surrogate is
                            // not a description we should act on
                            char::from_u32(code).ok_or(JsonError::BadEscape)?
                        }
                        _ => return Err(JsonError::BadEscape),
                    });
                }
                // raw control characters are not legal in a JSON string
                0x00..=0x1F => return Err(JsonError::Unexpected("control character in string")),
                _ => {
                    // copy the whole UTF-8 sequence
                    let start = self.i - 1;
                    let len = utf8_len(c);
                    if start + len > self.b.len() {
                        return Err(JsonError::Truncated);
                    }
                    let s = core::str::from_utf8(&self.b[start..start + len])
                        .map_err(|_| JsonError::Unexpected("bad utf-8"))?;
                    out.push_str(s);
                    self.i = start + len;
                }
            }
            if out.len() > MAX_STRING {
                return Err(JsonError::StringTooLong);
            }
        }
    }

    fn number(&mut self) -> Result<Json, JsonError> {
        let start = self.i;
        if self.peek() == Some(b'-') {
            self.i += 1;
        }
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.i += 1;
        }
        if self.peek() == Some(b'.') {
            self.i += 1;
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.i += 1;
            }
        }
        if matches!(self.peek(), Some(b'e') | Some(b'E')) {
            self.i += 1;
            if matches!(self.peek(), Some(b'+') | Some(b'-')) {
                self.i += 1;
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.i += 1;
            }
        }
        let text = core::str::from_utf8(&self.b[start..self.i]).map_err(|_| JsonError::BadNumber)?;
        text.parse::<f64>()
            .map(Json::Number)
            .map_err(|_| JsonError::BadNumber)
    }

    fn array(&mut self) -> Result<Json, JsonError> {
        self.eat(b'[')?;
        self.depth += 1;
        let mut out = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.i += 1;
            self.depth -= 1;
            return Ok(Json::Array(out));
        }
        loop {
            self.skip_ws();
            out.push(self.value()?);
            if out.len() > MAX_ITEMS {
                return Err(JsonError::TooManyItems);
            }
            self.skip_ws();
            match self.peek() {
                Some(b',') => self.i += 1,
                Some(b']') => {
                    self.i += 1;
                    self.depth -= 1;
                    return Ok(Json::Array(out));
                }
                _ => return Err(JsonError::Unexpected("expected , or ]")),
            }
        }
    }

    fn object(&mut self) -> Result<Json, JsonError> {
        self.eat(b'{')?;
        self.depth += 1;
        let mut out: Vec<(String, Json)> = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.i += 1;
            self.depth -= 1;
            return Ok(Json::Object(out));
        }
        loop {
            self.skip_ws();
            let key = self.string()?;
            self.skip_ws();
            self.eat(b':')?;
            self.skip_ws();
            let value = self.value()?;
            out.push((key, value));
            if out.len() > MAX_ITEMS {
                return Err(JsonError::TooManyItems);
            }
            self.skip_ws();
            match self.peek() {
                Some(b',') => self.i += 1,
                Some(b'}') => {
                    self.i += 1;
                    self.depth -= 1;
                    return Ok(Json::Object(out));
                }
                _ => return Err(JsonError::Unexpected("expected , or }")),
            }
        }
    }
}

const fn utf8_len(b: u8) -> usize {
    if b < 0x80 {
        1
    } else if b >> 5 == 0b110 {
        2
    } else if b >> 4 == 0b1110 {
        3
    } else {
        4
    }
}
