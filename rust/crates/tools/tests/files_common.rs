//! Purpose: verify real native Location resolution and approval ordering through tool ports.
//! Scope: live temp-directory paths and fixture authorization recorder.
//! Uses:
//! rust/crates/
//! |-- platform-api/src/fs.rs
//! |-- platform-native/src/fs/location.rs
//! |-- tool-api/src/context.rs
//! `-- tools/src/files/common.rs
//! External:
//! |-- async_trait::async_trait
//! `-- tempfile::tempdir
//! Used by:
//! `-- none, entrypoint
//! Invariants: no path resolution or approval helper reads target content.
//! Contract tests: this integration suite

use async_trait::async_trait;
use opencode_permissions::{AssertInput, PermissionError, Source};
use opencode_platform_api::fs::{
    LocationMutation, PathError, PathErrorReason, ResolveInput, ResolveKind,
};
use opencode_platform_native::fs::NativeLocationMutation;
use opencode_tool_api::{Authorizer, NoProgress, ToolContext};
use opencode_tools::files::common::{authorize_target, legacy_output};
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

fn input(path: impl Into<String>) -> ResolveInput {
    ResolveInput {
        path: path.into(),
        kind: None,
    }
}

#[tokio::test]
async fn existing_prospective_absolute_and_location_root() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("hello.txt"), "hello").unwrap();
    let resolver = NativeLocationMutation::new(temp.path()).await.unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    let existing = resolver.resolve(input("hello.txt")).await.unwrap();
    assert_eq!(existing.canonical, root.join("hello.txt"));
    assert_eq!(existing.resource, "hello.txt");
    assert!(existing.external_directory.is_none());
    let missing = resolver
        .resolve(input("new/nested/file.txt"))
        .await
        .unwrap();
    assert_eq!(missing.canonical, root.join("new/nested/file.txt"));
    assert_eq!(missing.resource, "new/nested/file.txt");
    assert!(missing.external_directory.is_none());
    let absolute = resolver
        .resolve(input(temp.path().join("new.txt").to_string_lossy()))
        .await
        .unwrap();
    assert_eq!(absolute.resource, "new.txt");
    assert!(absolute.external_directory.is_none());
    assert_eq!(resolver.resolve(input(".")).await.unwrap().resource, ".");
}

#[tokio::test]
async fn rejects_lexical_escape() {
    let temp = tempfile::tempdir().unwrap();
    let resolver = NativeLocationMutation::new(temp.path()).await.unwrap();
    assert!(matches!(
        resolver.resolve(input("../outside.txt")).await,
        Err(PathError::Invalid {
            reason: PathErrorReason::RelativeEscape,
            ..
        })
    ));
}

#[tokio::test]
async fn external_targets_approve_the_stable_existing_directory() {
    let temp = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("existing.txt"), "hello").unwrap();
    let resolver = NativeLocationMutation::new(temp.path()).await.unwrap();
    let root = std::fs::canonicalize(outside.path()).unwrap();
    for relative in ["existing.txt", "new/nested/file.txt"] {
        let target = resolver
            .resolve(input(outside.path().join(relative).to_string_lossy()))
            .await
            .unwrap();
        assert_eq!(target.canonical, root.join(relative));
        assert_eq!(target.resource, root.join(relative).to_string_lossy());
        let external = target.external_directory.unwrap();
        assert_eq!(external.directory, root);
        assert_eq!(external.resource, root.join("*").to_string_lossy());
        assert_eq!(external.save, external.resource);
    }
    let target = resolver
        .resolve(ResolveInput {
            path: outside.path().to_string_lossy().into(),
            kind: Some(ResolveKind::Directory),
        })
        .await
        .unwrap();
    assert_eq!(target.external_directory.unwrap().directory, root);
}

#[cfg(unix)]
#[tokio::test]
async fn symlinks_inside_are_followed_and_escape_is_rejected() {
    let temp = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::create_dir(temp.path().join("actual")).unwrap();
    std::os::unix::fs::symlink(temp.path().join("actual"), temp.path().join("linked")).unwrap();
    std::os::unix::fs::symlink(outside.path(), temp.path().join("escape")).unwrap();
    std::fs::write(outside.path().join("existing"), "secret").unwrap();
    let resolver = NativeLocationMutation::new(temp.path()).await.unwrap();
    assert_eq!(
        resolver
            .resolve(input("linked/new.txt"))
            .await
            .unwrap()
            .resource,
        "actual/new.txt"
    );
    for path in [
        "escape/new.txt",
        "escape/missing/descendant",
        "escape/existing",
    ] {
        assert!(
            matches!(
                resolver.resolve(input(path)).await,
                Err(PathError::Invalid {
                    reason: PathErrorReason::LocationEscape,
                    ..
                })
            ),
            "{path}"
        );
    }
}

#[derive(Default)]
struct Recorder {
    calls: Mutex<Vec<AssertInput>>,
    deny: Option<&'static str>,
}

#[async_trait]
impl Authorizer for Recorder {
    async fn assert(&self, input: AssertInput) -> Result<(), PermissionError> {
        let denied = self.deny == Some(input.action.as_str());
        self.calls.lock().unwrap().push(input);
        if denied {
            return Err(PermissionError::Declined);
        }
        Ok(())
    }
}

fn context(recorder: Arc<Recorder>) -> ToolContext {
    ToolContext {
        session_id: "session".into(),
        agent: "build".into(),
        assistant_message_id: "message".into(),
        tool_call_id: "call".into(),
        cancel: CancellationToken::new(),
        authorizer: recorder,
        progress: Arc::new(NoProgress),
    }
}

#[tokio::test]
async fn external_authorization_precedes_operation_and_preserves_source() {
    let temp = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let resolver = NativeLocationMutation::new(temp.path()).await.unwrap();
    let target = resolver
        .resolve(input(outside.path().join("new.txt").to_string_lossy()))
        .await
        .unwrap();
    let recorder = Arc::new(Recorder::default());
    let metadata = serde_json::json!({"diff":"preview"})
        .as_object()
        .unwrap()
        .clone();
    authorize_target(
        &target,
        "edit",
        &context(recorder.clone()),
        Some(metadata.clone()),
    )
    .await
    .unwrap();
    let calls = recorder.calls.lock().unwrap();
    assert_eq!(
        calls
            .iter()
            .map(|call| call.action.as_str())
            .collect::<Vec<_>>(),
        ["external_directory", "edit"]
    );
    assert_eq!(
        calls[0].resources,
        [target.external_directory.as_ref().unwrap().resource.clone()]
    );
    assert_eq!(calls[0].save.as_ref().unwrap(), &calls[0].resources);
    assert_eq!(calls[1].resources, [target.resource]);
    assert_eq!(calls[1].save, Some(vec!["*".into()]));
    assert_eq!(calls[1].metadata, Some(metadata));
    assert_eq!(
        calls[1].source,
        Some(Source::Tool {
            message_id: "message".into(),
            call_id: "call".into()
        })
    );
    assert_eq!(calls[1].session_id, "session");
    assert_eq!(calls[1].agent.as_deref(), Some("build"));
}

#[tokio::test]
async fn declined_external_approval_does_not_request_operation() {
    let temp = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let resolver = NativeLocationMutation::new(temp.path()).await.unwrap();
    let target = resolver
        .resolve(input(outside.path().join("new.txt").to_string_lossy()))
        .await
        .unwrap();
    let recorder = Arc::new(Recorder {
        deny: Some("external_directory"),
        ..Recorder::default()
    });
    let error = authorize_target(&target, "edit", &context(recorder.clone()), None)
        .await
        .unwrap_err();
    assert_eq!(error.message, "permission declined");
    assert_eq!(recorder.calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn internal_operation_does_not_request_external_approval() {
    let temp = tempfile::tempdir().unwrap();
    let resolver = NativeLocationMutation::new(temp.path()).await.unwrap();
    let target = resolver.resolve(input("new.txt")).await.unwrap();
    let recorder = Arc::new(Recorder {
        deny: Some("read"),
        ..Recorder::default()
    });
    assert!(
        authorize_target(&target, "read", &context(recorder.clone()), None)
            .await
            .is_err()
    );
    let calls = recorder.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].action, "read");
}

#[tokio::test]
async fn legacy_external_boundary_preserves_metadata_and_bypass() {
    use opencode_tools::files::common::authorize_legacy_external;
    use std::path::Path;
    let recorder = Arc::new(Recorder::default());
    let context = context(recorder.clone());
    for (target, bypass) in [
        ("/work/project/file", false),
        ("/work/other/file", false),
        ("/outside/file", true),
    ] {
        assert!(
            !authorize_legacy_external(
                Path::new("/work/project"),
                Path::new("/work"),
                Path::new(target),
                ResolveKind::File,
                bypass,
                &context
            )
            .await
            .unwrap()
        );
    }
    assert!(recorder.calls.lock().unwrap().is_empty());
    assert!(
        authorize_legacy_external(
            Path::new("/work/project"),
            Path::new("/work"),
            Path::new("/outside/file"),
            ResolveKind::File,
            false,
            &context
        )
        .await
        .unwrap()
    );
    assert!(
        authorize_legacy_external(
            Path::new("/work/project"),
            Path::new("/work"),
            Path::new("/outside/folder"),
            ResolveKind::Directory,
            false,
            &context
        )
        .await
        .unwrap()
    );
    let calls = recorder.calls.lock().unwrap();
    assert_eq!(calls[0].resources, ["/outside/*"]);
    assert_eq!(
        calls[0].metadata,
        Some(
            serde_json::json!({"filepath":"/outside/file","parentDir":"/outside"})
                .as_object()
                .unwrap()
                .clone()
        )
    );
    assert_eq!(calls[1].resources, ["/outside/folder/*"]);
}

#[test]
fn legacy_output_keeps_wire_metadata_and_model_text() {
    let output = legacy_output(
        "src/file.ts".into(),
        serde_json::json!({"diagnostics":{}}),
        "Edit applied successfully.".into(),
    );
    assert_eq!(
        output.structured,
        serde_json::json!({"title":"src/file.ts","metadata":{"diagnostics":{}},"output":"Edit applied successfully."})
    );
    assert_eq!(
        output.content,
        vec![opencode_tool_api::ToolContent::text(
            "Edit applied successfully."
        )]
    );
}
