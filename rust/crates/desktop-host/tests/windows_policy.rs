//! Purpose: contract tests for windows policy state machine.
//! Scope: tests for external URL, navigation, permissions, and recovery.
//! Uses:
//! rust/crates/
//! `-- desktop-host/src/windows/policy.rs
//! External:
//! `-- tokio::test
//! Used by:
//! `-- none, entrypoint
//! Invariants: tests cover all TS baseline cases exactly.
//! Contract tests: this file

#[cfg(test)]
mod tests {
    use opencode_desktop_host::windows::policy::*;

    #[test]
    fn test_resolve_external_url_web() {
        let res = resolve_external_url("https://example.com/a?b=c");
        assert!(
            matches!(res, ExternalUrlResolution::Allowed { url, protocol: ExternalProtocol::Https } if url == "https://example.com/a?b=c")
        );

        let res = resolve_external_url("http://example.com");
        assert!(
            matches!(res, ExternalUrlResolution::Allowed { url, protocol: ExternalProtocol::Http } if url == "http://example.com/")
        );
    }

    #[test]
    fn test_resolve_external_url_mailto() {
        let res = resolve_external_url("mailto:hello@opencode.ai");
        assert!(
            matches!(res, ExternalUrlResolution::Allowed { url, protocol: ExternalProtocol::Mailto } if url == "mailto:hello@opencode.ai")
        );
    }

    #[test]
    fn test_resolve_external_url_denied() {
        assert!(matches!(
            resolve_external_url("file:///tmp/index.html"),
            ExternalUrlResolution::Denied
        ));
        assert!(matches!(
            resolve_external_url("javascript:alert(1)"),
            ExternalUrlResolution::Denied
        ));
        assert!(matches!(
            resolve_external_url("data:text/html,hello"),
            ExternalUrlResolution::Denied
        ));
        assert!(matches!(
            resolve_external_url("not a url"),
            ExternalUrlResolution::Denied
        ));
    }

    #[test]
    fn test_resolve_local_file_path() {
        #[cfg(unix)]
        {
            let res = resolve_local_file_path("file:///tmp/example.html");
            assert!(
                matches!(res, LocalFilePathResolution::Allowed { path } if path == "/tmp/example.html")
            );
        }
        #[cfg(windows)]
        {
            let res = resolve_local_file_path("file:///C:/tmp/example.html");
            assert!(
                matches!(res, LocalFilePathResolution::Allowed { path } if path.contains("C:\\tmp\\example.html") || path.contains("C:/tmp/example.html"))
            );
        }

        assert!(matches!(
            resolve_local_file_path("file://example.com/share/index.html"),
            LocalFilePathResolution::Denied
        ));
        assert!(matches!(
            resolve_local_file_path("https://example.com/index.html"),
            LocalFilePathResolution::Denied
        ));
    }

    #[test]
    fn test_is_trusted_renderer_url() {
        let config = RendererConfig::default();
        assert!(is_trusted_renderer_url(
            "oc://renderer/index.html",
            &config,
            true
        ));
        assert!(is_trusted_renderer_url(
            "oc://renderer/app.js",
            &config,
            false
        ));
        assert!(!is_trusted_renderer_url(
            "oc://renderer/app.js",
            &config,
            true
        ));
        assert!(!is_trusted_renderer_url(
            "https://example.com",
            &config,
            false
        ));

        let dev_config = RendererConfig {
            dev_url_origin: Some("http://localhost:5173".to_string()),
            ..Default::default()
        };
        assert!(is_trusted_renderer_url(
            "http://localhost:5173/index.html",
            &dev_config,
            true
        ));
        assert!(!is_trusted_renderer_url(
            "http://localhost:5174/index.html",
            &dev_config,
            true
        ));
    }

    #[test]
    fn test_evaluate_navigation() {
        let config = RendererConfig::default();
        assert_eq!(
            evaluate_navigation("oc://renderer/index.html", &config),
            NavigationDecision::AllowInWindow
        );
        assert_eq!(
            evaluate_navigation("https://example.com", &config),
            NavigationDecision::DenyAndOpenExternal {
                url: "https://example.com".to_string()
            }
        );
    }

    #[test]
    fn test_evaluate_permission() {
        let config = RendererConfig::default();
        assert_eq!(
            evaluate_permission("clipboard-sanitized-write", "oc://renderer", 1, 1, &config),
            PermissionDecision::Allow
        );
        assert_eq!(
            evaluate_permission("notifications", "oc://renderer", 1, 1, &config),
            PermissionDecision::Allow
        );
        assert_eq!(
            evaluate_permission("geolocation", "oc://renderer", 1, 1, &config),
            PermissionDecision::Deny
        );
        assert_eq!(
            evaluate_permission("notifications", "https://evil.com", 1, 1, &config),
            PermissionDecision::Deny
        );
        assert_eq!(
            evaluate_permission("notifications", "oc://renderer", 1, 2, &config),
            PermissionDecision::Deny
        );
    }

    #[tokio::test]
    async fn test_unresponsive_sampler() {
        let config = UnresponsiveSamplerConfig {
            sample_interval_ms: 100,
            sample_period_ms: 300,
        };
        let mut sampler = UnresponsiveSampler::new(config);

        assert!(sampler.start());
        assert!(!sampler.start()); // Already sampling

        sampler.collect_sample(Some("stack1".to_string()));
        sampler.collect_sample(Some("stack1".to_string()));
        sampler.collect_sample(Some("stack2".to_string()));

        let (was_sampling, entries) = sampler.stop_and_flush();
        assert!(was_sampling);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0], ("stack1".to_string(), 2));
        assert_eq!(entries[1], ("stack2".to_string(), 1));

        let (was_sampling2, empty) = sampler.stop_and_flush();
        assert!(!was_sampling2);
        assert!(empty.is_empty());
    }

    #[tokio::test]
    async fn test_unresponsive_sampler_dev_tools() {
        let config = UnresponsiveSamplerConfig::default();
        let mut sampler = UnresponsiveSampler::new(config);
        sampler.set_dev_tools_open(true);

        assert!(!sampler.start()); // Should not start if dev tools are open
    }

    #[test]
    fn test_recovery_event_load_failed() {
        let mut state = RecoveryState::Idle;
        let event = RecoveryEvent::LoadFailed {
            error_code: -3,
            error_description: "Aborted".to_string(),
            validated_url: "oc://renderer".to_string(),
            is_main_frame: true,
        };
        let result = evaluate_recovery_event(&event, &mut state);
        assert!(matches!(result, RecoveryResult::NoAction));

        let event2 = RecoveryEvent::LoadFailed {
            error_code: -6,
            error_description: "File not found".to_string(),
            validated_url: "oc://renderer".to_string(),
            is_main_frame: true,
        };
        let result2 = evaluate_recovery_event(&event2, &mut state);
        assert!(matches!(
            result2,
            RecoveryResult::ShowDialog {
                wait_allowed: false,
                ..
            }
        ));
    }

    #[test]
    fn test_recovery_event_render_process_gone() {
        let mut state = RecoveryState::Idle;
        let event = RecoveryEvent::RenderProcessGone {
            reason: "crashed".to_string(),
            exit_code: Some(139),
        };
        let result = evaluate_recovery_event(&event, &mut state);
        assert!(matches!(
            result,
            RecoveryResult::ShowDialog {
                wait_allowed: false,
                ..
            }
        ));
        assert!(matches!(
            state,
            RecoveryState::ShowingDialog {
                wait_allowed: false
            }
        ));
    }

    #[test]
    fn test_recovery_event_unresponsive_and_responsive() {
        let mut state = RecoveryState::Idle;
        let event = RecoveryEvent::Unresponsive;
        let result = evaluate_recovery_event(&event, &mut state);
        assert!(matches!(
            result,
            RecoveryResult::ShowDialog {
                wait_allowed: true,
                ..
            }
        ));
        assert!(matches!(
            state,
            RecoveryState::ShowingDialog { wait_allowed: true }
        ));

        let event2 = RecoveryEvent::Responsive;
        let result2 = evaluate_recovery_event(&event2, &mut state);
        assert!(matches!(
            result2,
            RecoveryResult::StopSamplingAndFlush { .. }
        ));
        assert!(matches!(state, RecoveryState::Idle));
    }

    #[test]
    fn test_recovery_action_evaluation() {
        let result = evaluate_recovery_action(RecoveryAction::Relaunch, false, false);
        assert!(matches!(
            result,
            RecoveryResult::ExecuteAction(RecoveryAction::Relaunch)
        ));

        let result = evaluate_recovery_action(RecoveryAction::Quit, false, false);
        assert!(matches!(
            result,
            RecoveryResult::ExecuteAction(RecoveryAction::Quit)
        ));

        let result = evaluate_recovery_action(RecoveryAction::KeepWaiting, true, false);
        assert!(matches!(result, RecoveryResult::NoAction));

        let result = evaluate_recovery_action(RecoveryAction::ExportLogs, true, true);
        assert!(matches!(result, RecoveryResult::StartSampling));

        let result = evaluate_recovery_action(RecoveryAction::ExportLogs, false, true);
        assert!(matches!(result, RecoveryResult::NoAction));
    }
}
