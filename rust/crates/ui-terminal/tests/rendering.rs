//! Purpose: verify markdown, diff, tool and streamed ANSI frames through public APIs.
//! Scope: fixture-free renderer and vt100 consumer tests.
//! Uses:
//! rust/crates/
//! |-- ui-terminal/src/prompt/mod.rs
//! |-- ui-terminal/src/rendering.rs
//! `-- ui-terminal/src/timeline/mod.rs
//! External:
//! |-- ratatui::{style, text}
//! |-- serde_json::json
//! `-- vt100::Parser
//! Used by:
//! `-- none, entrypoint
//! Invariants: assertions observe rendered cells and emitted ANSI, not copied logic.
//! Contract tests: this file
use opencode_ui_terminal::{
    prompt::Editor,
    rendering::{FrameInput, Renderer, compose, frame},
    timeline::{
        Message, Part, Timeline, diff, markdown,
        tool::{self, Status, Tool},
    },
};
use ratatui::{
    style::{Color, Modifier},
    text::Line,
};
use serde_json::json;
fn text(lines: &[Line<'_>]) -> String {
    lines
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}
fn tool(status: Status) -> Tool {
    Tool {
        name: "websearch".into(),
        status,
        title: None,
        input: json!({"query":"rust"}),
        structured: json!({"provider":"exa"}),
        output: "one\ntwo\nthree\nfour".into(),
        error: None,
        expanded: false,
    }
}
#[test]
fn markdown_preserves_nested_styles_code_lists_links_and_tables() {
    let rows = markdown::render(
        "# Title\n\n**bold *nested*** and `code` [link](https://example.org)\n\n- [x] done\n- next\n\n| A | B |\n|---|---|\n| 中 | é |\n\n```rust\nfn main() {}\n```",
    );
    let visible = text(&rows);
    for expected in [
        "Title",
        "bold nested",
        "code",
        "link",
        "[x] done",
        "next",
        "A │ B",
        "中 │ é",
        "fn main() {}",
    ] {
        assert!(visible.contains(expected), "{expected}: {visible}");
    }
    let spans = rows.iter().flat_map(|line| &line.spans).collect::<Vec<_>>();
    assert!(spans.iter().any(|span| {
        span.content == "nested"
            && span
                .style
                .add_modifier
                .contains(Modifier::BOLD | Modifier::ITALIC)
    }));
    assert!(
        spans
            .iter()
            .any(|span| span.content == "code" && span.style.fg == Some(Color::Yellow))
    );
    assert!(spans.iter().any(
        |span| span.content == "link" && span.style.add_modifier.contains(Modifier::UNDERLINED)
    ));
}
#[test]
fn markdown_unclosed_fences_remain_visible_during_stream() {
    for prefix in [
        "```rust\nfn",
        "```rust\nfn main()",
        "```rust\nfn main() {}\n```",
    ] {
        assert!(text(&markdown::render(prefix)).contains("fn"));
    }
}
#[test]
fn diff_tracks_line_numbers_colors_and_missing_newlines() {
    let rows = diff::render("same\nold\n", "same\nnew");
    let visible = text(&rows);
    assert!(visible.contains("   1    1   same"));
    assert!(visible.contains("   2      - old"));
    assert!(visible.contains("        2 + new"));
    assert!(visible.contains("\\ No newline at end of file"));
    assert!(rows.iter().any(|line| line.style.fg == Some(Color::Red)));
    assert!(rows.iter().any(|line| line.style.fg == Some(Color::Green)));
}
#[test]
fn pending_metadata_stays_hidden_and_errors_and_output_expand() {
    let mut model = tool(Status::Pending);
    assert!(tool::display_metadata(&model).is_none());
    assert!(text(&tool::render(&model)).starts_with("○ Web Search rust"));
    model.status = Status::Running;
    assert!(text(&tool::render(&model)).starts_with("◉ Exa Web Search rust"));
    assert!(text(&tool::render(&model)).contains("… 1 more lines"));
    model.expanded = true;
    assert!(text(&tool::render(&model)).contains("four"));
    model.status = Status::Error;
    model.error = Some("permission denied".into());
    assert!(text(&tool::render(&model)).starts_with("✗ Exa"));
    assert!(
        tool::render(&model).iter().any(
            |line| line.to_string() == "permission denied" && line.style.fg == Some(Color::Red)
        )
    );
    model.structured = json!([]);
    assert!(tool::display_metadata(&model).is_none());
}
#[test]
fn timeline_stream_does_not_duplicate_prefixes_and_preserves_parts() {
    let mut timeline = Timeline::default();
    timeline.push("user", "hello");
    timeline.append("assistant", "**hel");
    timeline.append("assistant", "lo** 中");
    assert_eq!(timeline.messages().len(), 2);
    assert_eq!(
        timeline.messages()[1].parts,
        [Part::Text("**hello** 中".into())]
    );
    timeline.push_message(Message {
        role: "assistant".into(),
        parts: vec![
            Part::Reasoning("thinking".into()),
            Part::Tool(tool(Status::Completed)),
            Part::Diff {
                before: "a\n".into(),
                after: "b\n".into(),
            },
        ],
    });
    let rows = timeline.lines(20);
    assert!(text(&rows).contains("hello 中"));
    assert!(text(&rows).contains("thinking"));
    assert!(text(&rows).contains("Exa Web Search"));
    timeline.clear();
    assert!(timeline.lines(20).is_empty());
}
#[test]
fn ansi_frame_renders_wide_cells_cursor_and_incremental_updates() {
    let mut timeline = Timeline::default();
    timeline.push("assistant", "**hello** 中文");
    let mut editor = Editor::new();
    editor.set_text("a中e\u{301}");
    let mut renderer = Renderer::default();
    let input = FrameInput {
        width: 32,
        height: 12,
        editor: &editor,
        timeline: &timeline,
        header: "Header",
        status: "Status",
        overlay: &[],
        scroll: 0,
    };
    let first = renderer.render(&input).unwrap();
    let mut terminal = vt100::Parser::new(12, 32, 0);
    terminal.process(first.as_bytes());
    let screen = terminal.screen();
    assert!(
        screen.contents().contains("hello 中文"),
        "{}",
        screen.contents()
    );
    assert!(screen.contents().contains("> a中e\u{301}"));
    assert_eq!(screen.cursor_position(), (10, 6));
    let unchanged = renderer.render(&input).unwrap();
    assert!(unchanged.len() < first.len() / 2);
    assert!(!unchanged.contains("hello"));
    timeline.append("assistant", " added");
    let updated = renderer
        .render(&FrameInput {
            width: 32,
            height: 12,
            editor: &editor,
            timeline: &timeline,
            header: "Header",
            status: "Status",
            overlay: &[],
            scroll: 0,
        })
        .unwrap();
    terminal.process(updated.as_bytes());
    assert!(terminal.screen().contents().contains("hello 中文 added"));
}
#[test]
fn resize_overlay_and_small_viewports_are_bounded() {
    let mut timeline = Timeline::default();
    timeline.push("user", "long content 中文");
    let editor = Editor::new();
    for width in 0..5 {
        for height in 0..5 {
            let rendered = compose(&FrameInput {
                width,
                height,
                editor: &editor,
                timeline: &timeline,
                header: "Header",
                status: "Status",
                overlay: &["Overlay".into()],
                scroll: usize::MAX,
            });
            assert!(rendered.cursor.0 < width.max(1));
            assert!(rendered.cursor.1 < height.max(1));
            assert!(!rendered.cursor_visible);
        }
    }
}
#[test]
fn provider_text_cannot_emit_terminal_controls() {
    let mut timeline = Timeline::default();
    timeline.push("assistant", "hello\u{1b}[2Jworld\u{7}");
    let editor = Editor::new();
    let output = frame(&FrameInput {
        width: 40,
        height: 10,
        editor: &editor,
        timeline: &timeline,
        header: "H",
        status: "S",
        overlay: &[],
        scroll: 0,
    })
    .unwrap();
    assert!(!output.contains("hello\u{1b}[2J"));
    assert!(!output.contains('\u{7}'));
    let mut terminal = vt100::Parser::new(10, 40, 0);
    terminal.process(output.as_bytes());
    assert!(terminal.screen().contents().contains("hello[2Jworld"));
}
