//! Purpose: compose editor, timeline and overlays into a ratatui buffer and ANSI frame.
//! Scope: pure frame production; caller owns terminal lifecycle and stdout.
//! Uses:
//! rust/crates/
//! |-- ui-terminal/src/prompt/mod.rs
//! `-- ui-terminal/src/timeline/mod.rs
//! External:
//! `-- ratatui::{backend, buffer, layout, text, widgets}
//! Used by:
//! rust/crates/
//! `-- ui-terminal/src/lib.rs
//! Invariants: terminal controls originate only in the backend, never model text.
//! Contract tests: ui-terminal/tests/rendering.rs
use crate::{
    prompt::Editor,
    timeline::{Timeline, safe_text},
};
use ratatui::{
    backend::{Backend, CrosstermBackend},
    buffer::Buffer,
    layout::Rect,
    text::Line,
    widgets::{Paragraph, Widget},
};

pub struct FrameInput<'a> {
    pub width: u16,
    pub height: u16,
    pub editor: &'a Editor,
    pub timeline: &'a Timeline,
    pub header: &'a str,
    pub status: &'a str,
    pub overlay: &'a [String],
    /// Rows above the live end. Zero follows streaming output.
    pub scroll: usize,
}
pub struct RenderedFrame {
    pub buffer: Buffer,
    pub cursor: (u16, u16),
    pub cursor_visible: bool,
}

pub fn compose(input: &FrameInput<'_>) -> RenderedFrame {
    let width = input.width.max(1);
    let height = input.height.max(1);
    let mut buffer = Buffer::empty(Rect::new(0, 0, width, height));
    let editor = input.editor.layout(width.saturating_sub(2).max(1));
    let prompt_height = (editor.lines.len().min(usize::from(height / 3)).max(1) as u16).min(height);
    let prompt_y = height.saturating_sub(prompt_height + 1);
    let timeline_height = prompt_y.saturating_sub(1);
    Paragraph::new(safe_text(input.header)).render(Rect::new(0, 0, width, 1), &mut buffer);
    let lines = input.timeline.lines(width);
    let end = lines.len().saturating_sub(input.scroll);
    let start = end.saturating_sub(usize::from(timeline_height));
    if timeline_height > 0 {
        Paragraph::new(lines[start..end].to_vec())
            .render(Rect::new(0, 1, width, timeline_height), &mut buffer);
    }
    let editor_scroll =
        usize::from(editor.cursor.1).saturating_sub(usize::from(prompt_height).saturating_sub(1));
    let prompt_lines = editor
        .lines
        .into_iter()
        .skip(editor_scroll)
        .map(|mut line| {
            line.spans.insert(0, "> ".into());
            line
        })
        .collect::<Vec<_>>();
    Paragraph::new(prompt_lines).render(Rect::new(0, prompt_y, width, prompt_height), &mut buffer);
    Paragraph::new(safe_text(input.status)).render(Rect::new(0, height - 1, width, 1), &mut buffer);
    if !input.overlay.is_empty() {
        let overlay_width = width.saturating_sub(4).max(1);
        let overlay_height = (input
            .overlay
            .len()
            .min(usize::from(height.saturating_sub(2))) as u16)
            .max(1);
        let area = Rect::new(
            (width - overlay_width) / 2,
            (height - overlay_height) / 2,
            overlay_width,
            overlay_height,
        );
        ratatui::widgets::Clear.render(area, &mut buffer);
        Paragraph::new(
            input
                .overlay
                .iter()
                .map(|line| Line::raw(safe_text(line)))
                .collect::<Vec<_>>(),
        )
        .render(area, &mut buffer);
    }
    RenderedFrame {
        buffer,
        cursor: (
            (editor.cursor.0 + 2).min(width - 1),
            prompt_y
                .saturating_add(editor.cursor.1.saturating_sub(editor_scroll as u16))
                .min(height - 1),
        ),
        cursor_visible: input.overlay.is_empty(),
    }
}
/// Keeps the previous screen so streaming updates emit only changed cells.
#[derive(Default)]
pub struct Renderer {
    previous: Option<Buffer>,
}
impl Renderer {
    pub fn render(&mut self, input: &FrameInput<'_>) -> std::io::Result<String> {
        let rendered = compose(input);
        let mut output = Vec::new();
        {
            let mut backend = CrosstermBackend::new(&mut output);
            backend.hide_cursor()?;
            let reset = self
                .previous
                .as_ref()
                .is_none_or(|old| old.area != rendered.buffer.area);
            if reset {
                backend.clear()?;
                self.previous = Some(Buffer::empty(rendered.buffer.area));
            }
            // Buffer::diff skips continuation cells belonging to wide graphemes.
            let previous = self.previous.as_ref().expect("initialized buffer");
            backend.draw(previous.diff(&rendered.buffer).into_iter())?;
            if rendered.cursor_visible {
                backend.set_cursor_position(rendered.cursor)?;
                backend.show_cursor()?;
            }
        }
        self.previous = Some(rendered.buffer);
        String::from_utf8(output).map_err(std::io::Error::other)
    }
    pub fn invalidate(&mut self) {
        self.previous = None;
    }
}
/// Fully paints a frame; no alternate-screen or raw-mode changes are performed here.
pub fn frame(input: &FrameInput<'_>) -> std::io::Result<String> {
    Renderer::default().render(input)
}
