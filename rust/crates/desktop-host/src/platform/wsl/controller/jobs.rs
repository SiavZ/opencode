//! Purpose: the exclusive, cancellable WSL jobs of `WslServersController` (`runJob` callers in
//! `wsl/servers.ts:311-420`): runtime probe, distro lists, WSL/distro/opencode installs and
//! parallel addable-distro probing.
//! Scope: `impl WslServersController` continuation; state transitions go through `update`.
//! Uses:
//! rust/crates/
//! |-- desktop-host/src/platform/wsl/controller.rs
//! |-- desktop-host/src/platform/wsl/mod.rs
//! |-- desktop-host/src/platform/wsl/policy.rs
//! `-- desktop-host/src/platform/wsl/runtime.rs
//! External:
//! `-- futures::future::{join, join_all}
//! Used by:
//! rust/crates/
//! `-- desktop-host/src/platform/wsl/controller.rs
//! Invariants: every job runs under `run_job` (a new job cancels the previous one); cancelled jobs
//! resolve `Ok(())`; addable probing checks opencode only for distros that can execute, bash and curl.
//! Contract tests: desktop-host/tests/wsl_servers.rs

use super::WslServersController;
use crate::platform::wsl::policy::{
    expect_opencode_version, pending_restart_after_wsl_install, wsl_server_id_to_restart,
};
use crate::platform::wsl::runtime::{self, RunOptions, WslError, summarize};
use crate::platform::wsl::{WslDistroProbe, WslJob, messages};

impl WslServersController {
    pub async fn probe_runtime(&self) -> Result<(), WslError> {
        self.run_job(WslJob::Runtime { started_at: now() }, async |cancel| {
            let runtime =
                runtime::probe_runtime(self.0.runner.as_ref(), &RunOptions::with_cancel(&cancel))
                    .await;
            self.update(|state| {
                state.pending_restart = state.pending_restart && !runtime.available;
                state.runtime = Some(runtime);
            });
            Ok(())
        })
        .await
        .map(drop)
    }

    pub async fn refresh_distros(&self) -> Result<(), WslError> {
        self.run_job(WslJob::Distros { started_at: now() }, async |cancel| {
            self.refresh_lists(&RunOptions::with_cancel(&cancel)).await
        })
        .await
        .map(drop)
    }

    async fn refresh_lists(&self, options: &RunOptions) -> Result<(), WslError> {
        let runner = self.0.runner.as_ref();
        let (installed, online) = futures::future::join(
            runtime::list_installed(runner, options),
            runtime::list_online(runner, options),
        )
        .await;
        let (installed, online) = (installed?, online?);
        self.update(|state| {
            state.installed = installed;
            state.online = online;
        });
        Ok(())
    }

    pub async fn install_wsl(&self) -> Result<(), WslError> {
        self.run_job(WslJob::InstallWsl { started_at: now() }, async |cancel| {
            let options = RunOptions::with_cancel(&cancel);
            let result =
                runtime::install_wsl_runtime_elevated(self.0.runner.as_ref(), &options).await?;
            if result.code != Some(0) {
                return Err(WslError::Message(summary_or(
                    &result.stderr,
                    &result.stdout,
                    messages::INSTALL_WSL,
                )));
            }
            let runtime = runtime::probe_runtime(self.0.runner.as_ref(), &options).await;
            self.update(|state| {
                state.pending_restart = pending_restart_after_wsl_install(&runtime);
                state.runtime = Some(runtime);
            });
            Ok(())
        })
        .await
        .map(drop)
    }

    pub async fn install_distro(&self, name: &str) -> Result<(), WslError> {
        let job = WslJob::InstallDistro {
            distro: name.to_string(),
            started_at: now(),
        };
        self.run_job(job, async |cancel| {
            let options = RunOptions::with_cancel(&cancel);
            let result = runtime::install_distro(self.0.runner.as_ref(), name, &options).await?;
            if result.code != Some(0) {
                return Err(WslError::Message(summary_or(
                    &result.stderr,
                    &result.stdout,
                    &messages::install_distro(name),
                )));
            }
            self.refresh_lists(&options).await?;
            let probe = runtime::probe_distro(self.0.runner.as_ref(), name, &options).await?;
            self.update(|state| state.distro_probes.insert(name.to_string(), probe));
            Ok(())
        })
        .await
        .map(drop)
    }

    /// `probeAddable`: probe unknown distros in parallel, then check opencode in parallel for
    /// ready distros that have no check yet.
    pub async fn probe_addable(&self, distros: &[String]) -> Result<(), WslError> {
        if distros.is_empty() {
            return Ok(());
        }
        let job = WslJob::ProbeAddable {
            distros: distros.to_vec(),
            started_at: now(),
        };
        self.run_job(job, async |cancel| {
            let options = RunOptions::with_cancel(&cancel);
            let mut unique: Vec<String> = Vec::new();
            distros
                .iter()
                .filter(|d| !unique.contains(d))
                .cloned()
                .collect::<Vec<_>>()
                .into_iter()
                .for_each(|d| {
                    if !unique.contains(&d) {
                        unique.push(d)
                    }
                });
            let unprobed: Vec<String> = {
                let state = self.state();
                unique
                    .iter()
                    .filter(|d| state.distro_probes.get(d).is_none())
                    .cloned()
                    .collect()
            };
            let probes = futures::future::join_all(
                unprobed
                    .iter()
                    .map(|d| runtime::probe_distro(self.0.runner.as_ref(), d, &options)),
            )
            .await;
            let probes = probes.into_iter().collect::<Result<Vec<_>, _>>()?;
            if !probes.is_empty() {
                self.update(|state| {
                    unprobed
                        .iter()
                        .zip(probes)
                        .for_each(|(d, p)| state.distro_probes.insert(d.clone(), p))
                });
            }
            let unchecked: Vec<String> = {
                let state = self.state();
                unique
                    .iter()
                    .filter(|d| state.distro_probes.get(d).is_some_and(distro_probe_ready))
                    .filter(|d| state.opencode_checks.get(d).is_none())
                    .cloned()
                    .collect()
            };
            let checks = futures::future::join_all(
                unchecked.iter().map(|d| self.check_opencode(d, &options)),
            )
            .await;
            let checks = checks.into_iter().collect::<Result<Vec<_>, _>>()?;
            if !checks.is_empty() {
                self.update(|state| {
                    unchecked
                        .iter()
                        .zip(checks)
                        .for_each(|(d, c)| state.opencode_checks.insert(d.clone(), c))
                });
            }
            Ok(())
        })
        .await
        .map(drop)
    }

    pub async fn install_opencode(&self, name: &str) -> Result<(), WslError> {
        let job = WslJob::InstallOpencode {
            distro: name.to_string(),
            started_at: now(),
        };
        self.run_job(job, async |cancel| {
            let options = RunOptions::with_cancel(&cancel);
            let result = runtime::install_opencode(
                self.0.runner.as_ref(),
                &self.0.app_version,
                name,
                &options,
            )
            .await?;
            if result.code != Some(0) {
                return Err(WslError::Message(summary_or(
                    &result.stderr,
                    &result.stdout,
                    messages::INSTALL_OPENCODE,
                )));
            }
            let check = self.check_opencode(name, &options).await?;
            let installed = check.version.clone();
            self.update(|state| state.opencode_checks.insert(name.to_string(), check));
            expect_opencode_version(installed.as_deref(), &self.0.app_version, name)
                .map_err(WslError::Message)?;
            if let Some(id) = wsl_server_id_to_restart(&self.state().servers, name) {
                self.start_server(&id).await;
            }
            Ok(())
        })
        .await
        .map(drop)
    }

    pub async fn open_terminal(&self, name: &str) -> Result<(), WslError> {
        runtime::open_terminal(self.0.runner.as_ref(), Some(name)).await
    }
}

fn distro_probe_ready(probe: &WslDistroProbe) -> bool {
    probe.can_execute && probe.has_bash && probe.has_curl
}

fn summary_or(stderr: &str, stdout: &str, fallback: &str) -> String {
    let summary = summarize(if stderr.is_empty() { stdout } else { stderr });
    if summary.is_empty() {
        return fallback.to_string();
    }
    summary
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}
