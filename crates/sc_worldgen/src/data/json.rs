//! 世界生成数据的严格 JSON 取值解析。
//!
//! Strict JSON value parsing for worldgen data.
//!
//! `serde_json::Value` 会静默合并重复的对象键，因此世界生成文件改走
//! [`parse_strict`]：它拒绝重复键、强制嵌套深度预算、为每个错误报告
//! JSON 指针风格的路径，且不保留键序语义（对象字段仅按名称寻址）。
//!
//! `serde_json::Value` collapses duplicate object keys silently, so worldgen
//! files go through [`parse_strict`] instead: it rejects duplicate keys,
//! enforces a nesting-depth budget, reports a JSON-pointer-style path for
//! every error and preserves no key-order semantics (object fields are
//! addressed by name only).

/// 解析后的严格 JSON 取值。对象保持插入顺序（每个键至多出现一次——重复键
/// 是硬错误），数字为 f64。
///
/// A parsed strict JSON value. Objects keep their insertion order (each key
/// appears at most once — duplicates are a hard error) and numbers are f64.
#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    /// 按名称查找字段（仅对象）。
    ///
    /// Field lookup by name (objects only).
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(fields) => fields.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Json::Num(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }

    /// 用于坐标/计数字段的整数提取（`3` 与 `3.0` 均可；非整数或越界取值为
    /// `None`）。
    ///
    /// Integral extraction for coordinate/count fields (`3` and `3.0` both
    /// accepted; non-integral or out-of-range values are `None`).
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Json::Num(n) if n.fract() == 0.0 && n.abs() <= 9.007_199_254_740_992e15 => {
                Some(*n as i64)
            }
            _ => None,
        }
    }

    pub fn is_obj(&self) -> bool {
        matches!(self, Json::Obj(_))
    }
}

/// 严格解析失败，附带发生处的 JSON 路径。
///
/// A strict parse failure with the JSON path where it happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonError {
    /// JSON 指针风格路径（`$.worldgen[0].size`）。
    ///
    /// JSON-pointer-style path (`$.worldgen[0].size`).
    pub path: String,
    pub detail: String,
}

impl std::fmt::Display for JsonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.path, self.detail)
    }
}

impl std::error::Error for JsonError {}

/// 接受的最大嵌套深度。真实世界生成文件远低于此；该上限把恶意输入变成
/// 有界失败。
///
/// Maximum nesting depth accepted. Real worldgen files stay far below this;
/// the bound turns hostile inputs into bounded failures.
pub const MAX_JSON_DEPTH: usize = 256;

/// 解析严格 JSON。树中任何位置的重复键都会被拒绝。
///
/// Parses strict JSON. Duplicate keys anywhere in the tree are rejected.
pub fn parse_strict(bytes: &[u8]) -> Result<Json, JsonError> {
    let mut parser = Parser { bytes, pos: 0, depth: 0 };
    parser.skip_ws();
    let value = parser.parse_value("$")?;
    parser.skip_ws();
    if parser.pos != parser.bytes.len() {
        return Err(parser.error("$", "trailing bytes after the JSON document"));
    }
    Ok(value)
}

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
    depth: usize,
}

impl<'a> Parser<'a> {
    fn error(&self, path: &str, detail: &str) -> JsonError {
        JsonError { path: path.to_string(), detail: detail.to_string() }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.pos += 1;
        }
    }

    fn expect(&mut self, byte: u8, path: &str) -> Result<(), JsonError> {
        if self.peek() == Some(byte) {
            self.pos += 1;
            Ok(())
        } else {
            Err(self.error(path, &format!("expected '{}'", byte as char)))
        }
    }

    fn parse_value(&mut self, path: &str) -> Result<Json, JsonError> {
        self.depth += 1;
        if self.depth > MAX_JSON_DEPTH {
            self.depth -= 1;
            return Err(self.error(path, "nesting depth exceeds the budget"));
        }
        let result = match self.peek() {
            Some(b'{') => self.parse_object(path),
            Some(b'[') => self.parse_array(path),
            Some(b'"') => self.parse_string(path).map(Json::Str),
            Some(b't') => self.parse_literal(path, "true", Json::Bool(true)),
            Some(b'f') => self.parse_literal(path, "false", Json::Bool(false)),
            Some(b'n') => self.parse_literal(path, "null", Json::Null),
            Some(c) if c == b'-' || c.is_ascii_digit() => self.parse_number(path),
            _ => Err(self.error(path, "expected a JSON value")),
        };
        self.depth -= 1;
        result
    }

    fn parse_object(&mut self, path: &str) -> Result<Json, JsonError> {
        self.expect(b'{', path)?;
        let mut fields: Vec<(String, Json)> = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(Json::Obj(fields));
        }
        loop {
            self.skip_ws();
            let key = self.parse_string(path)?;
            self.skip_ws();
            self.expect(b':', path)?;
            self.skip_ws();
            let child_path = format!("{path}.{key}");
            let value = self.parse_value(&child_path)?;
            if fields.iter().any(|(k, _)| *k == key) {
                return Err(JsonError {
                    path: child_path,
                    detail: "duplicate object key".to_string(),
                });
            }
            fields.push((key, value));
            self.skip_ws();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(Json::Obj(fields));
                }
                _ => return Err(self.error(path, "expected ',' or '}' in object")),
            }
        }
    }

    fn parse_array(&mut self, path: &str) -> Result<Json, JsonError> {
        self.expect(b'[', path)?;
        let mut items = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(Json::Arr(items));
        }
        let mut index = 0usize;
        loop {
            self.skip_ws();
            let value = self.parse_value(&format!("{path}[{index}]"))?;
            items.push(value);
            index += 1;
            self.skip_ws();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b']') => {
                    self.pos += 1;
                    return Ok(Json::Arr(items));
                }
                _ => return Err(self.error(path, "expected ',' or ']' in array")),
            }
        }
    }

    fn parse_string(&mut self, path: &str) -> Result<String, JsonError> {
        self.expect(b'"', path)?;
        let mut out = String::new();
        loop {
            match self.peek() {
                None => return Err(self.error(path, "unterminated string")),
                Some(b'"') => {
                    self.pos += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.pos += 1;
                    let escape = self.peek().ok_or_else(|| self.error(path, "bad escape"))?;
                    self.pos += 1;
                    match escape {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{0008}'),
                        b'f' => out.push('\u{000C}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let code = self.parse_hex4(path)?;
                            // Surrogate pairs are rejected: worldgen data is
                            // plain ASCII/latin and lone surrogates are not
                            // representable in Rust strings.
                            if (0xD800..0xE000).contains(&code) {
                                return Err(self.error(path, "surrogate escapes are not supported"));
                            }
                            out.push(char::from_u32(u32::from(code)).ok_or_else(|| {
                                self.error(path, "invalid unicode escape")
                            })?);
                        }
                        _ => return Err(self.error(path, "unknown escape")),
                    }
                }
                Some(c) if c < 0x20 => {
                    return Err(self.error(path, "raw control character in string"))
                }
                Some(c) => {
                    // Consume one UTF-8 code point verbatim.
                    let start = self.pos;
                    self.pos += 1;
                    while self.peek().is_some_and(|b| (b & 0xC0) == 0x80) {
                        self.pos += 1;
                    }
                    let slice = &self.bytes[start..self.pos];
                    out.push_str(std::str::from_utf8(slice).map_err(|_| {
                        self.error(path, "invalid UTF-8 in string")
                    })?);
                    let _ = c;
                }
            }
        }
    }

    fn parse_hex4(&mut self, path: &str) -> Result<u16, JsonError> {
        let mut value: u16 = 0;
        for _ in 0..4 {
            let c = self.peek().ok_or_else(|| self.error(path, "bad unicode escape"))?;
            let digit = (c as char).to_digit(16).ok_or_else(|| {
                self.error(path, "bad unicode escape digit")
            })?;
            value = value
                .checked_mul(16)
                .and_then(|v| v.checked_add(digit as u16))
                .ok_or_else(|| self.error(path, "bad unicode escape"))?;
            self.pos += 1;
        }
        Ok(value)
    }

    fn parse_number(&mut self, path: &str) -> Result<Json, JsonError> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        // Strict JSON: no leading zeros ("01" and "-01" are invalid).
        match self.peek() {
            Some(b'0') => {
                self.pos += 1;
                if self.peek().is_some_and(|b| b.is_ascii_digit()) {
                    return Err(self.error(path, "leading zeros are not allowed"));
                }
            }
            Some(c) if c.is_ascii_digit() => {
                while self.peek().is_some_and(|b| b.is_ascii_digit()) {
                    self.pos += 1;
                }
            }
            _ => return Err(self.error(path, "expected a digit")),
        }
        if self.peek() == Some(b'.') {
            self.pos += 1;
            if !self.peek().is_some_and(|b| b.is_ascii_digit()) {
                return Err(self.error(path, "expected digits after the decimal point"));
            }
            while self.peek().is_some_and(|b| b.is_ascii_digit()) {
                self.pos += 1;
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.pos += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.pos += 1;
            }
            if !self.peek().is_some_and(|b| b.is_ascii_digit()) {
                return Err(self.error(path, "expected digits in the exponent"));
            }
            while self.peek().is_some_and(|b| b.is_ascii_digit()) {
                self.pos += 1;
            }
        }
        let text = std::str::from_utf8(&self.bytes[start..self.pos])
            .map_err(|_| self.error(path, "invalid number bytes"))?;
        // Reject Rust-isms like leading '+', bare '.', '1.' — std f64 parse
        // already rejects those, so this is just error-context plumbing.
        let value: f64 = text
            .parse()
            .map_err(|_| self.error(path, &format!("invalid number '{text}'")))?;
        if !value.is_finite() {
            return Err(self.error(path, &format!("non-finite number '{text}'")));
        }
        Ok(Json::Num(value))
    }

    fn parse_literal(&mut self, path: &str, literal: &str, value: Json) -> Result<Json, JsonError> {
        if self.bytes[self.pos..].starts_with(literal.as_bytes()) {
            self.pos += literal.len();
            Ok(value)
        } else {
            Err(self.error(path, &format!("expected '{literal}'")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nested_structures_with_paths() {
        let json = parse_strict(br#"{"a": [1, -2.5e2, {"b": "x"}], "c": true, "d": null}"#).unwrap();
        assert_eq!(json.get("c"), Some(&Json::Bool(true)));
        let arr = json.get("a").unwrap();
        match arr {
            Json::Arr(items) => {
                assert_eq!(items[0], Json::Num(1.0));
                assert_eq!(items[1], Json::Num(-250.0));
                assert_eq!(
                    items[2].get("b"),
                    Some(&Json::Str("x".to_string()))
                );
            }
            other => panic!("expected array, got {other:?}"),
        }
    }

    #[test]
    fn duplicate_keys_are_rejected_with_a_path() {
        let error = parse_strict(br#"{"a": 1, "b": {"a": 2, "a": 3}}"#).unwrap_err();
        assert_eq!(error.path, "$.b.a");
        assert!(error.detail.contains("duplicate"));
    }

    #[test]
    fn depth_budget_is_enforced() {
        let mut deep = String::new();
        for _ in 0..(MAX_JSON_DEPTH + 8) {
            deep.push('[');
        }
        for _ in 0..(MAX_JSON_DEPTH + 8) {
            deep.push(']');
        }
        let error = parse_strict(deep.as_bytes()).unwrap_err();
        assert!(error.detail.contains("depth"), "got: {error}");
    }

    #[test]
    fn syntax_errors_carry_paths() {
        assert!(parse_strict(br#"{"a": }"#).unwrap_err().path == "$.a");
        assert!(parse_strict(br#"[1, 2"#).unwrap_err().detail.contains("']'"));
        assert!(parse_strict(b"{} trailing").unwrap_err().detail.contains("trailing"));
        assert!(parse_strict(br#"{"a": 01}"#).is_err());
        assert!(parse_strict(br#"{"a": 1.}"#).is_err());
        assert!(parse_strict(br#"{"a":tru}"#).is_err());
    }

    #[test]
    fn integral_extraction_accepts_only_integral_finite_values() {
        assert_eq!(parse_strict(b"3").unwrap().as_i64(), Some(3));
        assert_eq!(parse_strict(b"3.0").unwrap().as_i64(), Some(3));
        assert_eq!(parse_strict(b"-64").unwrap().as_i64(), Some(-64));
        assert_eq!(parse_strict(b"3.5").unwrap().as_i64(), None);
        assert_eq!(parse_strict(b"\"3\"").unwrap().as_i64(), None);
    }
}
