//! Purpose: JSONC tokenizer ported from jsonc-parser 3.3.1 `impl/scanner.js`, over UTF-16 code
//! units so offsets, lines and columns match the TypeScript baseline exactly.
//! Scope: pure lexer.
//! Uses:
//! `-- none
//! External:
//! `-- i64::from
//! Used by:
//! rust/crates/
//! |-- config/src/jsonc/format.rs
//! |-- config/src/jsonc/mod.rs
//! `-- config/src/jsonc/visit.rs
//! Invariants: token kinds and scan errors mirror the JS numeric enums; offsets are UTF-16 units.
//! Contract tests: config/tests/jsonc.rs

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Token {
    OpenBrace,
    CloseBrace,
    OpenBracket,
    CloseBracket,
    Comma,
    Colon,
    Null,
    True,
    False,
    String,
    Number,
    LineComment,
    BlockComment,
    LineBreak,
    Trivia,
    Unknown,
    Eof,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanError {
    None,
    UnexpectedEndOfComment,
    UnexpectedEndOfString,
    UnexpectedEndOfNumber,
    InvalidUnicode,
    InvalidEscapeCharacter,
    InvalidCharacter,
}

pub struct Scanner<'a> {
    text: &'a [u16],
    pos: usize,
    value: Vec<u16>,
    token_offset: usize,
    token: Token,
    error: ScanError,
}

const LF: u16 = 10;
const CR: u16 = 13;

pub fn is_line_break(ch: u16) -> bool {
    ch == LF || ch == CR
}

fn is_white_space(ch: u16) -> bool {
    ch == 32 || ch == 9
}

fn is_digit(ch: u16) -> bool {
    (48..=57).contains(&ch)
}

impl<'a> Scanner<'a> {
    pub fn new(text: &'a [u16]) -> Self {
        Self {
            text,
            pos: 0,
            value: Vec::new(),
            token_offset: 0,
            token: Token::Unknown,
            error: ScanError::None,
        }
    }

    fn at(&self, pos: usize) -> Option<u16> {
        self.text.get(pos).copied()
    }

    pub fn token(&self) -> Token {
        self.token
    }

    pub fn error(&self) -> ScanError {
        self.error
    }

    pub fn offset(&self) -> usize {
        self.token_offset
    }

    pub fn length(&self) -> usize {
        self.pos - self.token_offset
    }

    pub fn value(&self) -> String {
        String::from_utf16_lossy(&self.value)
    }

    fn scan_hex_digits(&mut self, count: usize) -> i64 {
        let mut digits = 0;
        let mut value: i64 = 0;
        while digits < count {
            let Some(ch) = self.at(self.pos) else { break };
            let digit = match ch {
                48..=57 => ch - 48,
                65..=70 => ch - 65 + 10,
                97..=102 => ch - 97 + 10,
                _ => break,
            };
            value = value * 16 + i64::from(digit);
            self.pos += 1;
            digits += 1;
        }
        if digits < count { -1 } else { value }
    }

    fn scan_number(&mut self) -> Vec<u16> {
        let start = self.pos;
        if self.at(self.pos) == Some(48) {
            self.pos += 1;
        } else {
            self.pos += 1;
            while self.at(self.pos).is_some_and(is_digit) {
                self.pos += 1;
            }
        }
        if self.at(self.pos) == Some(46) {
            self.pos += 1;
            if self.at(self.pos).is_some_and(is_digit) {
                self.pos += 1;
                while self.at(self.pos).is_some_and(is_digit) {
                    self.pos += 1;
                }
            } else {
                self.error = ScanError::UnexpectedEndOfNumber;
                return self.text[start..self.pos].to_vec();
            }
        }
        let mut end = self.pos;
        if matches!(self.at(self.pos), Some(69) | Some(101)) {
            self.pos += 1;
            if matches!(self.at(self.pos), Some(43) | Some(45)) {
                self.pos += 1;
            }
            if self.at(self.pos).is_some_and(is_digit) {
                self.pos += 1;
                while self.at(self.pos).is_some_and(is_digit) {
                    self.pos += 1;
                }
                end = self.pos;
            } else {
                self.error = ScanError::UnexpectedEndOfNumber;
            }
        }
        self.text[start..end].to_vec()
    }

    fn scan_string(&mut self) -> Vec<u16> {
        let len = self.text.len();
        let mut result = Vec::new();
        let mut start = self.pos;
        loop {
            if self.pos >= len {
                result.extend_from_slice(&self.text[start..self.pos]);
                self.error = ScanError::UnexpectedEndOfString;
                break;
            }
            let ch = self.text[self.pos];
            if ch == 34 {
                result.extend_from_slice(&self.text[start..self.pos]);
                self.pos += 1;
                break;
            }
            if ch == 92 {
                result.extend_from_slice(&self.text[start..self.pos]);
                self.pos += 1;
                if self.pos >= len {
                    self.error = ScanError::UnexpectedEndOfString;
                    break;
                }
                let ch2 = self.text[self.pos];
                self.pos += 1;
                match ch2 {
                    34 => result.push(34),
                    92 => result.push(92),
                    47 => result.push(47),
                    98 => result.push(8),
                    102 => result.push(12),
                    110 => result.push(10),
                    114 => result.push(13),
                    116 => result.push(9),
                    117 => {
                        let ch3 = self.scan_hex_digits(4);
                        if ch3 >= 0 {
                            result.push(ch3 as u16);
                        } else {
                            self.error = ScanError::InvalidUnicode;
                        }
                    }
                    _ => self.error = ScanError::InvalidEscapeCharacter,
                }
                start = self.pos;
                continue;
            }
            if ch <= 0x1f {
                if is_line_break(ch) {
                    result.extend_from_slice(&self.text[start..self.pos]);
                    self.error = ScanError::UnexpectedEndOfString;
                    break;
                }
                self.error = ScanError::InvalidCharacter;
            }
            self.pos += 1;
        }
        result
    }

    pub fn scan(&mut self) -> Token {
        self.token = self.scan_next();
        self.token
    }

    fn scan_next(&mut self) -> Token {
        let len = self.text.len();
        self.value.clear();
        self.error = ScanError::None;
        self.token_offset = self.pos;
        if self.pos >= len {
            self.token_offset = len;
            return Token::Eof;
        }
        let mut code = self.text[self.pos];
        if is_white_space(code) {
            loop {
                self.pos += 1;
                self.value.push(code);
                match self.at(self.pos) {
                    Some(next) if is_white_space(next) => code = next,
                    _ => break,
                }
            }
            return Token::Trivia;
        }
        if is_line_break(code) {
            self.pos += 1;
            self.value.push(code);
            if code == CR && self.at(self.pos) == Some(LF) {
                self.pos += 1;
                self.value.push(LF);
            }
            return Token::LineBreak;
        }
        match code {
            123 => {
                self.pos += 1;
                Token::OpenBrace
            }
            125 => {
                self.pos += 1;
                Token::CloseBrace
            }
            91 => {
                self.pos += 1;
                Token::OpenBracket
            }
            93 => {
                self.pos += 1;
                Token::CloseBracket
            }
            58 => {
                self.pos += 1;
                Token::Colon
            }
            44 => {
                self.pos += 1;
                Token::Comma
            }
            34 => {
                self.pos += 1;
                self.value = self.scan_string();
                Token::String
            }
            47 => self.scan_comment(),
            45 => {
                self.value.push(code);
                self.pos += 1;
                if self.pos == len || !is_digit(self.text[self.pos]) {
                    return Token::Unknown;
                }
                let number = self.scan_number();
                self.value.extend(number);
                Token::Number
            }
            48..=57 => {
                self.value = self.scan_number();
                Token::Number
            }
            _ => self.scan_word(),
        }
    }

    fn scan_comment(&mut self) -> Token {
        let len = self.text.len();
        let start = self.pos;
        if self.at(self.pos + 1) == Some(47) {
            self.pos += 2;
            while self.pos < len && !is_line_break(self.text[self.pos]) {
                self.pos += 1;
            }
            self.value = self.text[start..self.pos].to_vec();
            return Token::LineComment;
        }
        if self.at(self.pos + 1) == Some(42) {
            self.pos += 2;
            let safe_length = len.saturating_sub(1);
            let mut closed = false;
            while self.pos < safe_length {
                let ch = self.text[self.pos];
                if ch == 42 && self.at(self.pos + 1) == Some(47) {
                    self.pos += 2;
                    closed = true;
                    break;
                }
                self.pos += 1;
                if ch == CR && self.at(self.pos) == Some(LF) {
                    self.pos += 1;
                }
            }
            if !closed {
                self.pos += 1;
                self.error = ScanError::UnexpectedEndOfComment;
            }
            let end = self.pos.min(len);
            self.value = self.text[start..end].to_vec();
            return Token::BlockComment;
        }
        self.value.push(47);
        self.pos += 1;
        Token::Unknown
    }

    fn scan_word(&mut self) -> Token {
        let len = self.text.len();
        while self.pos < len && is_unknown_content(self.text[self.pos]) {
            self.pos += 1;
        }
        if self.token_offset != self.pos {
            self.value = self.text[self.token_offset..self.pos].to_vec();
            return match String::from_utf16_lossy(&self.value).as_str() {
                "true" => Token::True,
                "false" => Token::False,
                "null" => Token::Null,
                _ => Token::Unknown,
            };
        }
        self.value.push(self.text[self.pos]);
        self.pos += 1;
        Token::Unknown
    }
}

fn is_unknown_content(code: u16) -> bool {
    if is_white_space(code) || is_line_break(code) {
        return false;
    }
    !matches!(code, 125 | 93 | 123 | 91 | 34 | 58 | 44 | 47)
}
