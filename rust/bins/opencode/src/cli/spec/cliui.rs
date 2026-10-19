//! Purpose: byte-compatible port of the cliui 9 layout engine and wrap-ansi 9 hard wrapping that
//! yargs 18 uses to render help (node_modules cliui/build/lib/index.js, wrap-ansi/index.js).
//! Scope: pure text layout.
//! Uses:
//! `-- none
//! External:
//! `-- str::to_string
//! Used by:
//! rust/bins/
//! |-- opencode/src/cli/spec/help.rs
//! `-- opencode/src/cli/spec/mod.rs
//! Invariants: only the features yargs help uses (span/div, padding, width, right align, inline
//! render of a row into a preceding span line); text is plain (no ANSI), width is char count.
//! Contract tests: opencode/tests/cli_help.rs
#[derive(Debug, Clone, Default)]
pub struct Col {
    pub text: String,
    /// top, right, bottom, left.
    pub padding: [usize; 4],
    pub width: Option<usize>,
    pub align_right: bool,
}

impl Col {
    pub fn text(text: impl Into<String>) -> Self {
        Col {
            text: text.into(),
            ..Col::default()
        }
    }
}

#[derive(Debug)]
struct Row {
    cols: Vec<Col>,
    span: bool,
}

#[derive(Debug)]
struct Line {
    text: String,
    span: bool,
    hidden: bool,
}

#[derive(Debug)]
pub struct Ui {
    width: usize,
    rows: Vec<Row>,
}

fn width(s: &str) -> usize {
    s.chars().count()
}

impl Ui {
    pub fn new(width: usize) -> Self {
        Ui {
            width,
            rows: Vec::new(),
        }
    }

    /// `ui.div(...cols)`; an empty call behaves like cliui: one `''` row plus an empty row.
    pub fn div(&mut self, cols: Vec<Col>) {
        if cols.is_empty() {
            self.rows.push(Row {
                cols: vec![Col::text("")],
                span: false,
            });
        }
        self.rows.push(Row { cols, span: false });
    }

    pub fn span(&mut self, cols: Vec<Col>) {
        self.rows.push(Row { cols, span: true });
    }

    pub fn render(&self) -> String {
        let mut lines: Vec<Line> = Vec::new();
        for row in &self.rows {
            self.row_to_string(row, &mut lines);
        }
        let visible: Vec<&str> = lines
            .iter()
            .filter(|l| !l.hidden)
            .map(|l| l.text.as_str())
            .collect();
        visible.join("\n")
    }

    fn row_to_string(&self, row: &Row, lines: &mut Vec<Line>) {
        let widths = self.column_widths(row);
        let rrows = rasterize(row, &widths);
        for (r, rrow) in rrows.iter().enumerate() {
            let mut out = String::new();
            for (c, cell) in rrow.iter().enumerate() {
                let col = &row.cols[c];
                let wrap_width = negate_padding(col, widths[c]);
                let mut ts = cell.clone();
                if wrap_width > width(&ts) {
                    ts.push_str(&" ".repeat(wrap_width - width(&ts)));
                }
                if col.align_right {
                    ts = align_right(&ts, wrap_width);
                    if width(&ts) < wrap_width {
                        ts.push_str(&" ".repeat(widths[c].saturating_sub(width(&ts) + 1)));
                    }
                }
                out.push_str(&" ".repeat(col.padding[3]));
                out.push_str(&ts);
                out.push_str(&" ".repeat(col.padding[1]));
                if r == 0 && !lines.is_empty() {
                    out = render_inline(out, lines);
                }
            }
            lines.push(Line {
                text: out.trim_end_matches(' ').to_string(),
                span: row.span,
                hidden: false,
            });
        }
    }

    fn column_widths(&self, row: &Row) -> Vec<usize> {
        let unset = row.cols.iter().filter(|c| c.width.is_none()).count();
        let fixed: usize = row.cols.iter().filter_map(|c| c.width).sum();
        let remaining = self.width as i64 - fixed as i64;
        let unset_width = if unset > 0 {
            (remaining as f64 / unset as f64).floor() as i64
        } else {
            0
        };
        row.cols
            .iter()
            .map(|c| match c.width {
                Some(w) => w,
                None => {
                    let min = 1 + c.padding[3] + c.padding[1];
                    (unset_width.max(min as i64)) as usize
                }
            })
            .collect()
    }
}

fn negate_padding(col: &Col, width: usize) -> usize {
    width.saturating_sub(col.padding[3] + col.padding[1])
}

fn rasterize(row: &Row, widths: &[usize]) -> Vec<Vec<String>> {
    let mut rrows: Vec<Vec<String>> = Vec::new();
    for (c, col) in row.cols.iter().enumerate() {
        let wrapped_text = wrap_hard(&col.text, negate_padding(col, widths[c]));
        let mut wrapped: Vec<String> = wrapped_text.split('\n').map(str::to_string).collect();
        for _ in 0..col.padding[0] {
            wrapped.insert(0, String::new());
        }
        for _ in 0..col.padding[2] {
            wrapped.push(String::new());
        }
        for (r, s) in wrapped.into_iter().enumerate() {
            if rrows.len() <= r {
                rrows.push(Vec::new());
            }
            let rrow = &mut rrows[r];
            while rrow.len() < c {
                rrow.push(String::new());
            }
            rrow.push(s);
        }
    }
    rrows
}

fn align_right(s: &str, w: usize) -> String {
    let s = s.trim();
    let sw = width(s);
    if sw < w {
        return format!("{}{}", " ".repeat(w - sw), s);
    }
    s.to_string()
}

fn render_inline(source: String, lines: &mut [Line]) -> String {
    let Some(previous) = lines.last_mut() else {
        return source;
    };
    if !previous.span {
        return source;
    }
    let leading = source.len() - source.trim_start_matches(' ').len();
    let target = previous.text.trim_end().to_string();
    let target_width = width(&target);
    if leading < target_width {
        return source;
    }
    previous.hidden = true;
    format!(
        "{}{}{}",
        target,
        " ".repeat(leading - target_width),
        source.trim_start()
    )
}

/// wrap-ansi `exec(string, columns, { hard: true })` for plain text.
pub fn wrap_hard(text: &str, columns: usize) -> String {
    if text.trim().is_empty() {
        return String::new();
    }
    let words: Vec<&str> = text.split(' ').collect();
    let mut rows: Vec<String> = vec![String::new()];
    for (index, word) in words.iter().enumerate() {
        let last = rows.last_mut().expect("rows never empty");
        *last = last.trim_start().to_string();
        let mut row_length = width(last);
        if index != 0 && row_length > 0 {
            last.push(' ');
            row_length += 1;
        }
        let len = width(word);
        if len > columns {
            let remaining = columns as i64 - row_length as i64;
            let this_line =
                1 + ((len as i64 - remaining - 1) as f64 / columns as f64).floor() as i64;
            let next_line = ((len as i64 - 1) as f64 / columns as f64).floor() as i64;
            if next_line < this_line {
                rows.push(String::new());
            }
            wrap_word(&mut rows, word, columns);
            continue;
        }
        if row_length + len > columns && row_length > 0 && len > 0 {
            rows.push(String::new());
        }
        rows.last_mut().expect("rows never empty").push_str(word);
    }
    let rows: Vec<String> = rows.into_iter().map(|r| trim_spaces_right(&r)).collect();
    rows.join("\n")
}

fn wrap_word(rows: &mut Vec<String>, word: &str, columns: usize) {
    let chars: Vec<char> = word.chars().collect();
    let mut visible = width(rows.last().expect("rows never empty"));
    for (index, ch) in chars.iter().enumerate() {
        if visible < columns {
            rows.last_mut().expect("rows never empty").push(*ch);
        } else {
            rows.push(ch.to_string());
            visible = 0;
        }
        visible += 1;
        if visible == columns && index < chars.len() - 1 {
            rows.push(String::new());
            visible = 0;
        }
    }
    if visible == 0 && rows.len() > 1 && !rows.last().expect("rows never empty").is_empty() {
        let last = rows.pop().expect("checked");
        rows.last_mut().expect("checked").push_str(&last);
    }
}

fn trim_spaces_right(s: &str) -> String {
    let words: Vec<&str> = s.split(' ').collect();
    let mut last = words.len();
    while last > 0 && words[last - 1].is_empty() {
        last -= 1;
    }
    if last == words.len() {
        return s.to_string();
    }
    format!("{}{}", words[..last].join(" "), words[last..].join(""))
}
