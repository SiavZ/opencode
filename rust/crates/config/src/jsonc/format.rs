//! Purpose: range formatter ported from jsonc-parser 3.3.1 `impl/format.js`, used by `modify` so
//! comment-preserving config edits produce byte-identical text to the TypeScript writer.
//! Scope: pure text edits over UTF-16.
//! Uses:
//! rust/crates/
//! `-- config/src/jsonc/scanner.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! |-- config/src/jsonc/edit.rs
//! `-- config/src/jsonc/mod.rs
//! Invariants: only `keepLines: false`, `insertSpaces: true` paths are used by config writes, but
//! all branches of the baseline are kept so formatting stays faithful.
//! Contract tests: config/tests/jsonc.rs

use super::scanner::{Scanner, Token, is_line_break};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub offset: usize,
    pub length: usize,
    pub content: Vec<u16>,
}

#[derive(Debug, Clone, Copy)]
pub struct Options {
    pub tab_size: usize,
    pub insert_spaces: bool,
    pub keep_lines: bool,
    pub insert_final_newline: bool,
}

pub fn is_eol(text: &[u16], offset: usize) -> bool {
    text.get(offset).copied().is_some_and(is_line_break)
}

fn units(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

fn repeat(unit: &[u16], count: usize) -> Vec<u16> {
    unit.repeat(count)
}

/// `format(documentText, range, options)` for a given range.
pub fn format(document: &[u16], range: (usize, usize), options: Options) -> Vec<Edit> {
    let (range_start, range_length) = range;
    let range_end = range_start + range_length;
    let mut format_start = range_start;
    while format_start > 0 && !is_eol(document, format_start - 1) {
        format_start -= 1;
    }
    let mut end_offset = range_end;
    while end_offset < document.len() && !is_eol(document, end_offset) {
        end_offset += 1;
    }
    let format_text = &document[format_start..end_offset];
    let initial_indent = compute_indent_level(format_text, options);
    let eol = get_eol(document);
    let indent_value = if options.insert_spaces {
        repeat(&[32], options.tab_size.max(1))
    } else {
        vec![9]
    };

    let mut scanner = Scanner::new(format_text);
    let mut has_error = false;
    let mut line_breaks = 0usize;
    let mut indent_level: isize = 0;
    let mut edits = Vec::new();

    let new_lines_and_indent = |line_breaks: usize, level: isize| -> Vec<u16> {
        let count = (initial_indent as isize + level).max(0) as usize;
        let mut out = if line_breaks > 1 {
            repeat(&eol, line_breaks)
        } else {
            eol.clone()
        };
        out.extend(repeat(&indent_value, count));
        out
    };
    let scan_next =
        |scanner: &mut Scanner, line_breaks: &mut usize, has_error: &mut bool| -> Token {
            let mut token = scanner.scan();
            *line_breaks = 0;
            while matches!(token, Token::Trivia | Token::LineBreak) {
                if token == Token::LineBreak {
                    *line_breaks = if options.keep_lines {
                        *line_breaks + 1
                    } else {
                        1
                    };
                }
                token = scanner.scan();
            }
            *has_error =
                token == Token::Unknown || scanner.error() != super::scanner::ScanError::None;
            token
        };
    let add_edit = |edits: &mut Vec<Edit>,
                    has_error: bool,
                    text: Vec<u16>,
                    start: usize,
                    end: usize| {
        if !has_error && start < range_end && end > range_start && document[start..end] != text[..]
        {
            edits.push(Edit {
                offset: start,
                length: end - start,
                content: text,
            });
        }
    };

    let mut first = scan_next(&mut scanner, &mut line_breaks, &mut has_error);
    if options.keep_lines && line_breaks > 0 {
        add_edit(&mut edits, has_error, repeat(&eol, line_breaks), 0, 0);
    }
    if first != Token::Eof {
        let first_start = scanner.offset() + format_start;
        add_edit(
            &mut edits,
            has_error,
            repeat(&indent_value, initial_indent),
            format_start,
            first_start,
        );
    }
    while first != Token::Eof {
        let mut first_end = scanner.offset() + scanner.length() + format_start;
        let mut second = scan_next(&mut scanner, &mut line_breaks, &mut has_error);
        let mut replace: Vec<u16> = Vec::new();
        let mut needs_line_break = false;
        while line_breaks == 0 && matches!(second, Token::LineComment | Token::BlockComment) {
            let comment_start = scanner.offset() + format_start;
            add_edit(&mut edits, has_error, units(" "), first_end, comment_start);
            first_end = scanner.offset() + scanner.length() + format_start;
            needs_line_break = second == Token::LineComment;
            replace = if needs_line_break {
                new_lines_and_indent(line_breaks, indent_level)
            } else {
                Vec::new()
            };
            second = scan_next(&mut scanner, &mut line_breaks, &mut has_error);
        }
        if second == Token::CloseBrace || second == Token::CloseBracket {
            let open = if second == Token::CloseBrace {
                Token::OpenBrace
            } else {
                Token::OpenBracket
            };
            if first != open {
                indent_level -= 1;
            }
            if (options.keep_lines && line_breaks > 0) || (!options.keep_lines && first != open) {
                replace = new_lines_and_indent(line_breaks, indent_level);
            } else if options.keep_lines {
                replace = units(" ");
            }
        } else {
            match first {
                Token::OpenBracket | Token::OpenBrace | Token::Comma => {
                    if first != Token::Comma {
                        indent_level += 1;
                    }
                    replace = if (options.keep_lines && line_breaks > 0) || !options.keep_lines {
                        new_lines_and_indent(line_breaks, indent_level)
                    } else {
                        units(" ")
                    };
                }
                Token::LineComment => replace = new_lines_and_indent(line_breaks, indent_level),
                Token::BlockComment => {
                    if line_breaks > 0 {
                        replace = new_lines_and_indent(line_breaks, indent_level);
                    } else if !needs_line_break {
                        replace = units(" ");
                    }
                }
                Token::Colon => {
                    if options.keep_lines && line_breaks > 0 {
                        replace = new_lines_and_indent(line_breaks, indent_level);
                    } else if !needs_line_break {
                        replace = units(" ");
                    }
                }
                Token::String => {
                    if options.keep_lines && line_breaks > 0 {
                        replace = new_lines_and_indent(line_breaks, indent_level);
                    } else if second == Token::Colon && !needs_line_break {
                        replace = Vec::new();
                    }
                }
                Token::Null
                | Token::True
                | Token::False
                | Token::Number
                | Token::CloseBrace
                | Token::CloseBracket => {
                    if options.keep_lines && line_breaks > 0 {
                        replace = new_lines_and_indent(line_breaks, indent_level);
                    } else if matches!(second, Token::LineComment | Token::BlockComment)
                        && !needs_line_break
                    {
                        replace = units(" ");
                    } else if second != Token::Comma && second != Token::Eof {
                        has_error = true;
                    }
                }
                Token::Unknown => has_error = true,
                _ => {}
            }
            if line_breaks > 0 && matches!(second, Token::LineComment | Token::BlockComment) {
                replace = new_lines_and_indent(line_breaks, indent_level);
            }
        }
        if second == Token::Eof {
            replace = if options.keep_lines && line_breaks > 0 {
                new_lines_and_indent(line_breaks, indent_level)
            } else if options.insert_final_newline {
                eol.clone()
            } else {
                Vec::new()
            };
        }
        let second_start = scanner.offset() + format_start;
        add_edit(&mut edits, has_error, replace, first_end, second_start);
        first = second;
    }
    edits
}

fn compute_indent_level(content: &[u16], options: Options) -> usize {
    let tab = options.tab_size.max(1);
    let mut chars = 0;
    for &ch in content {
        match ch {
            32 => chars += 1,
            9 => chars += tab,
            _ => break,
        }
    }
    chars / tab
}

fn get_eol(text: &[u16]) -> Vec<u16> {
    for (index, &ch) in text.iter().enumerate() {
        if ch == 13 {
            return if text.get(index + 1) == Some(&10) {
                vec![13, 10]
            } else {
                vec![13]
            };
        }
        if ch == 10 {
            return vec![10];
        }
    }
    vec![10]
}
