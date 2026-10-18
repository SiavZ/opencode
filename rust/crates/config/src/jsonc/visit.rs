//! Purpose: JSONC parser ported from jsonc-parser 3.3.1 `visit`/`parseTree`, producing a syntax
//! tree with UTF-16 offsets plus the exact error list the TypeScript parser reports.
//! Scope: pure parser.
//! Uses:
//! rust/crates/
//! `-- config/src/jsonc/scanner.rs
//! External:
//! `-- serde_json::{Map, Number, Value}
//! Used by:
//! rust/crates/
//! |-- config/src/jsonc/edit.rs
//! `-- config/src/jsonc/mod.rs
//! Invariants: error recovery, skip sets and callback order match `impl/parser.js` `visit`.
//! Contract tests: config/tests/jsonc.rs

use super::scanner::{ScanError, Scanner, Token};
use serde_json::{Map, Number, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    InvalidSymbol,
    InvalidNumberFormat,
    PropertyNameExpected,
    ValueExpected,
    ColonExpected,
    CommaExpected,
    CloseBraceExpected,
    CloseBracketExpected,
    EndOfFileExpected,
    InvalidCommentToken,
    UnexpectedEndOfComment,
    UnexpectedEndOfString,
    UnexpectedEndOfNumber,
    InvalidUnicode,
    InvalidEscapeCharacter,
    InvalidCharacter,
}

impl ErrorCode {
    /// `printParseErrorCode`.
    pub fn name(self) -> &'static str {
        match self {
            Self::InvalidSymbol => "InvalidSymbol",
            Self::InvalidNumberFormat => "InvalidNumberFormat",
            Self::PropertyNameExpected => "PropertyNameExpected",
            Self::ValueExpected => "ValueExpected",
            Self::ColonExpected => "ColonExpected",
            Self::CommaExpected => "CommaExpected",
            Self::CloseBraceExpected => "CloseBraceExpected",
            Self::CloseBracketExpected => "CloseBracketExpected",
            Self::EndOfFileExpected => "EndOfFileExpected",
            Self::InvalidCommentToken => "InvalidCommentToken",
            Self::UnexpectedEndOfComment => "UnexpectedEndOfComment",
            Self::UnexpectedEndOfString => "UnexpectedEndOfString",
            Self::UnexpectedEndOfNumber => "UnexpectedEndOfNumber",
            Self::InvalidUnicode => "InvalidUnicode",
            Self::InvalidEscapeCharacter => "InvalidEscapeCharacter",
            Self::InvalidCharacter => "InvalidCharacter",
        }
    }
}

/// One parse error, offsets in UTF-16 code units like the TypeScript `ParseError`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParseError {
    pub error: ErrorCode,
    pub offset: usize,
    pub length: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    Object,
    Array,
    Property,
    String,
    Number,
    Boolean,
    Null,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub kind: NodeKind,
    pub offset: usize,
    pub length: usize,
    pub children: Vec<Node>,
    pub value: Value,
}

impl Node {
    fn new(kind: NodeKind, offset: usize) -> Self {
        Self {
            kind,
            offset,
            length: 0,
            children: Vec::new(),
            value: Value::Null,
        }
    }

    /// `getNodeValue`: duplicate keys keep their first position with the last value.
    pub fn to_value(&self) -> Value {
        match self.kind {
            NodeKind::Array => Value::Array(self.children.iter().map(Node::to_value).collect()),
            NodeKind::Object => {
                let mut map = Map::new();
                for property in &self.children {
                    if let (Some(key), Some(value)) =
                        (property.children.first(), property.children.get(1))
                    {
                        map.insert(
                            key.value.as_str().unwrap_or_default().to_string(),
                            value.to_value(),
                        );
                    }
                }
                Value::Object(map)
            }
            NodeKind::Property => Value::Null,
            _ => self.value.clone(),
        }
    }

    /// `findNodeAtLocation` for property names.
    pub fn find(&self, key: &str) -> Option<&Node> {
        if self.kind != NodeKind::Object {
            return None;
        }
        self.children
            .iter()
            .find(|property| {
                property.children.len() == 2 && property.children[0].value.as_str() == Some(key)
            })
            .map(|property| &property.children[1])
    }
}

/// Parse `text` (as UTF-16) into a tree, collecting errors. `allowTrailingComma` is always on
/// for config callers; `edit` passes `false` like `setProperty` does.
pub fn parse_tree(
    text: &[u16],
    errors: &mut Vec<ParseError>,
    allow_trailing_comma: bool,
) -> Option<Node> {
    let mut parser = Parser {
        scanner: Scanner::new(text),
        errors,
        stack: vec![Node::new(NodeKind::Array, 0)],
        allow_trailing_comma,
    };
    parser.run();
    parser
        .stack
        .into_iter()
        .next()
        .and_then(|root| root.children.into_iter().next())
}

struct Parser<'a, 'e> {
    scanner: Scanner<'a>,
    errors: &'e mut Vec<ParseError>,
    stack: Vec<Node>,
    allow_trailing_comma: bool,
}

impl Parser<'_, '_> {
    fn top(&mut self) -> &mut Node {
        let last = self.stack.len() - 1;
        &mut self.stack[last]
    }

    fn close_top(&mut self) {
        if self.stack.len() < 2 {
            return;
        }
        let Some(node) = self.stack.pop() else { return };
        self.top().children.push(node);
    }

    fn ensure_property_complete(&mut self, end: usize) {
        if self.top().kind == NodeKind::Property {
            let top = self.top();
            top.length = end - top.offset;
            self.close_top();
        }
    }

    fn on_value(&mut self, node: Node) {
        let end = node.offset + node.length;
        self.top().children.push(node);
        self.ensure_property_complete(end);
    }

    fn on_container_end(&mut self, property_first: bool) {
        let end = self.scanner.offset() + self.scanner.length();
        if property_first {
            self.ensure_property_complete(end);
        }
        let top = self.top();
        top.length = end - top.offset;
        self.close_top();
        self.ensure_property_complete(end);
    }

    fn scan_next(&mut self) -> Token {
        loop {
            let token = self.scanner.scan();
            match self.scanner.error() {
                ScanError::InvalidUnicode => self.handle_error(ErrorCode::InvalidUnicode, &[], &[]),
                ScanError::InvalidEscapeCharacter => {
                    self.handle_error(ErrorCode::InvalidEscapeCharacter, &[], &[])
                }
                ScanError::UnexpectedEndOfNumber => {
                    self.handle_error(ErrorCode::UnexpectedEndOfNumber, &[], &[])
                }
                ScanError::UnexpectedEndOfComment => {
                    self.handle_error(ErrorCode::UnexpectedEndOfComment, &[], &[])
                }
                ScanError::UnexpectedEndOfString => {
                    self.handle_error(ErrorCode::UnexpectedEndOfString, &[], &[])
                }
                ScanError::InvalidCharacter => {
                    self.handle_error(ErrorCode::InvalidCharacter, &[], &[])
                }
                ScanError::None => {}
            }
            match token {
                Token::LineComment | Token::BlockComment | Token::Trivia | Token::LineBreak => {}
                Token::Unknown => self.handle_error(ErrorCode::InvalidSymbol, &[], &[]),
                _ => return token,
            }
        }
    }

    fn handle_error(&mut self, error: ErrorCode, skip_until_after: &[Token], skip_until: &[Token]) {
        self.errors.push(ParseError {
            error,
            offset: self.scanner.offset(),
            length: self.scanner.length(),
        });
        if skip_until_after.is_empty() && skip_until.is_empty() {
            return;
        }
        let mut token = self.scanner.token();
        while token != Token::Eof {
            if skip_until_after.contains(&token) {
                self.scan_next();
                break;
            }
            if skip_until.contains(&token) {
                break;
            }
            token = self.scan_next();
        }
    }

    fn literal(&mut self, kind: NodeKind, value: Value) {
        let mut node = Node::new(kind, self.scanner.offset());
        node.length = self.scanner.length();
        node.value = value;
        self.on_value(node);
    }

    fn parse_literal(&mut self) -> bool {
        match self.scanner.token() {
            Token::Number => {
                let value = match js_number(&self.scanner.value()) {
                    Some(value) => value,
                    None => {
                        self.handle_error(ErrorCode::InvalidNumberFormat, &[], &[]);
                        Value::Number(Number::from(0))
                    }
                };
                self.literal(NodeKind::Number, value);
            }
            Token::Null => self.literal(NodeKind::Null, Value::Null),
            Token::True => self.literal(NodeKind::Boolean, Value::Bool(true)),
            Token::False => self.literal(NodeKind::Boolean, Value::Bool(false)),
            _ => return false,
        }
        self.scan_next();
        true
    }

    fn parse_property(&mut self) -> bool {
        const SKIP: &[Token] = &[Token::CloseBrace, Token::Comma];
        if self.scanner.token() != Token::String {
            self.handle_error(ErrorCode::PropertyNameExpected, &[], SKIP);
            return false;
        }
        let mut property = Node::new(NodeKind::Property, self.scanner.offset());
        let mut key = Node::new(NodeKind::String, self.scanner.offset());
        key.length = self.scanner.length();
        key.value = Value::String(self.scanner.value());
        property.children.push(key);
        self.stack.push(property);
        self.scan_next();
        if self.scanner.token() == Token::Colon {
            self.scan_next();
            if !self.parse_value() {
                self.handle_error(ErrorCode::ValueExpected, &[], SKIP);
            }
        } else {
            self.handle_error(ErrorCode::ColonExpected, &[], SKIP);
        }
        true
    }

    fn parse_object(&mut self) -> bool {
        self.stack
            .push(Node::new(NodeKind::Object, self.scanner.offset()));
        self.scan_next();
        let mut needs_comma = false;
        while !matches!(self.scanner.token(), Token::CloseBrace | Token::Eof) {
            if self.scanner.token() == Token::Comma {
                if !needs_comma {
                    self.handle_error(ErrorCode::ValueExpected, &[], &[]);
                }
                let offset = self.scanner.offset();
                self.ensure_property_complete(offset);
                self.scan_next();
                if self.scanner.token() == Token::CloseBrace && self.allow_trailing_comma {
                    break;
                }
            } else if needs_comma {
                self.handle_error(ErrorCode::CommaExpected, &[], &[]);
            }
            if !self.parse_property() {
                self.handle_error(
                    ErrorCode::ValueExpected,
                    &[],
                    &[Token::CloseBrace, Token::Comma],
                );
            }
            needs_comma = true;
        }
        self.on_container_end(true);
        if self.scanner.token() != Token::CloseBrace {
            self.handle_error(ErrorCode::CloseBraceExpected, &[Token::CloseBrace], &[]);
        } else {
            self.scan_next();
        }
        true
    }

    fn parse_array(&mut self) -> bool {
        self.stack
            .push(Node::new(NodeKind::Array, self.scanner.offset()));
        self.scan_next();
        let mut needs_comma = false;
        while !matches!(self.scanner.token(), Token::CloseBracket | Token::Eof) {
            if self.scanner.token() == Token::Comma {
                if !needs_comma {
                    self.handle_error(ErrorCode::ValueExpected, &[], &[]);
                }
                self.scan_next();
                if self.scanner.token() == Token::CloseBracket && self.allow_trailing_comma {
                    break;
                }
            } else if needs_comma {
                self.handle_error(ErrorCode::CommaExpected, &[], &[]);
            }
            if !self.parse_value() {
                self.handle_error(
                    ErrorCode::ValueExpected,
                    &[],
                    &[Token::CloseBracket, Token::Comma],
                );
            }
            needs_comma = true;
        }
        self.on_container_end(false);
        if self.scanner.token() != Token::CloseBracket {
            self.handle_error(ErrorCode::CloseBracketExpected, &[Token::CloseBracket], &[]);
        } else {
            self.scan_next();
        }
        true
    }

    fn parse_value(&mut self) -> bool {
        match self.scanner.token() {
            Token::OpenBracket => self.parse_array(),
            Token::OpenBrace => self.parse_object(),
            Token::String => {
                let value = Value::String(self.scanner.value());
                self.literal(NodeKind::String, value);
                self.scan_next();
                true
            }
            _ => self.parse_literal(),
        }
    }

    fn run(&mut self) {
        self.scan_next();
        if self.scanner.token() == Token::Eof {
            self.handle_error(ErrorCode::ValueExpected, &[], &[]);
            return;
        }
        if !self.parse_value() {
            self.handle_error(ErrorCode::ValueExpected, &[], &[]);
            return;
        }
        if self.scanner.token() != Token::Eof {
            self.handle_error(ErrorCode::EndOfFileExpected, &[], &[]);
        }
    }
}

/// `Number(text)` for scanner-produced numerals. Integral values inside the safe range are kept
/// as integers so serialization matches `JSON.stringify`.
pub fn js_number(text: &str) -> Option<Value> {
    let parsed: f64 = text.parse().ok()?;
    Some(number_value(parsed))
}

/// Convert an f64 to a JSON number the way `JSON.stringify` prints it: integral values below
/// 1e21 print without exponent, so keep them as integers.
pub fn number_value(value: f64) -> Value {
    if value.fract() == 0.0 && value.abs() < 1e21 {
        if value == 0.0 {
            return Value::Number(Number::from(0));
        }
        if value.abs() <= i64::MAX as f64 {
            return Value::Number(Number::from(value as i64));
        }
        if value > 0.0 && value <= u64::MAX as f64 {
            return Value::Number(Number::from(value as u64));
        }
    }
    Number::from_f64(value)
        .map(Value::Number)
        .unwrap_or(Value::Null)
}
