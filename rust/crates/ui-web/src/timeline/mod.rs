//! Purpose: render canonical projected messages and all part kinds safely.
//! Scope: session transcript, tool output and history pagination.
//! Uses:
//! `-- none
//! External:
//! `-- none
//! Used by:
//! `-- none
//! Invariants: user-controlled markdown is sanitized; keyed message identities survive updates.
//! Contract tests: browser acceptance; view-model/tests/projection.rs
use crate::state::Browser;
use leptos::prelude::*;
use opencode_view_model::projection::{PartKind, RenderPart, ToolStatus, project};

pub const DEFAULT_ROW_ESTIMATE_PX: f64 = 96.0;

#[component]
pub fn Timeline() -> impl IntoView {
    let browser = expect_context::<Browser>();
    let projection = Memo::new(move |_| {
        browser
            .state
            .with(|state| project(state.cache.source(&state.active)))
    });
    view! {
        <section class="oc-timeline" aria-label="Conversation" aria-live="polite">
            <Show when=move ||browser.state.with(|state|state.cache.get(&state.active).and_then(|entry|entry.cursor.as_ref()).is_some())>
                <button disabled=move ||browser.state.with(|state|state.loading) on:click=move |_| {
                    let cursor=browser.state.with(|state|state.cache.get(&state.active).and_then(|entry|entry.cursor.clone()));
                    browser.load(cursor);
                }>"Load earlier messages"</button>
            </Show>
            <For each=move ||projection.get().messages key=|message|message.id.clone() children=move |message| {
                let id=message.id.clone();
                let role=if message.is_user(){"You"}else{"Assistant"};
                view! { <article class="oc-message" data-message-id=message.id><h2>{role}</h2>
                    <For each=move ||projection.with(|projection|projection.parts_of(&id).to_vec()) key=|part|part.id.clone() children=move |part|view!{<Part part/>}/>
                </article> }
            }/>
            <Show when=move ||projection.with(|projection|projection.messages.is_empty())><p class="oc-empty">"Start a session to work on your project."</p></Show>
        </section>
    }
}

#[component]
fn Part(part: RenderPart) -> impl IntoView {
    match part.kind {
        PartKind::Text{text,..} => view!{<div class="oc-markdown" inner_html=crate::markdown::render(&text)/ >}.into_any(),
        PartKind::Reasoning{text,..} => view!{<details class="oc-reasoning"><summary>"Reasoning"</summary><div class="oc-markdown" inner_html=crate::markdown::render(&text)/></details>}.into_any(),
        PartKind::File{url,filename,mime,..} => {
            let safe = safe_url(&url);
            view!{<a class="oc-file" href=safe target="_blank" rel="noopener noreferrer">{filename.unwrap_or(mime)}</a>}.into_any()
        }
        PartKind::Agent{name} => view!{<span class="oc-agent">"@"{name}</span>}.into_any(),
        PartKind::Compaction{..} => view!{<p>"Conversation compacted"</p>}.into_any(),
        PartKind::Tool{tool,input,status,..} => {
            let (label,output)=match status {
                ToolStatus::Pending{raw} => ("Pending",raw),
                ToolStatus::Running => ("Running",String::new()),
                ToolStatus::Completed{output,..} => ("Completed",output),
                ToolStatus::Error{error} => ("Error",error),
            };
            view!{<details class="oc-tool" open=label=="Running"><summary>{tool}" · "{label}</summary><pre>{serde_json::to_string_pretty(&input).unwrap_or_default()}</pre><pre>{output}</pre></details>}.into_any()
        }
    }
}

pub fn safe_url(value: &str) -> Option<String> {
    if value.starts_with('/') && !value.starts_with("//") {
        return Some(value.into());
    }
    let url = url::Url::parse(value).ok()?;
    matches!(url.scheme(), "https" | "http" | "blob").then(|| value.into())
}
