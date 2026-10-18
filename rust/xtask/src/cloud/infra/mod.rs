//! Purpose: compose the stage plan in the same order as `sst.config.ts` run() (sst.config.ts:30-45).
//! Scope: pure plan construction.
//! Uses:
//! rust/xtask/
//! |-- src/cloud/infra/app.rs
//! |-- src/cloud/infra/console.rs
//! |-- src/cloud/infra/monitoring.rs
//! |-- src/cloud/infra/stage.rs
//! |-- src/cloud/infra/stats.rs
//! `-- src/cloud/model.rs
//! External:
//! `-- serde_json::json
//! Used by:
//! rust/xtask/
//! |-- src/cloud/mod.rs
//! `-- src/cloud/tests.rs
//! Invariants: stats only when deployAws, monitoring only for production/vimtor, outputs are
//! StatWorkerUrl, optional StatsUrl and AwsStage in that order.
//! Contract tests: xtask/src/cloud/tests.rs

pub mod app;
pub mod console;
pub mod monitoring;
pub mod stage;
pub mod stats;

use serde_json::json;

use crate::cloud::model::{Plan, Stage, reference};

pub fn build(stage: Stage) -> Plan {
    let mut plan = Plan::new(stage);
    // ES module evaluation order: stage.ts, app.ts, stats.ts (imports app/stage only), then
    // secret.ts as console.ts's first unevaluated import, then console.ts and enterprise.ts.
    stage::stage(&mut plan);
    app::app(&mut plan);
    let deploy_aws = plan.stage.deploy_aws();
    if deploy_aws {
        stats::stats(&mut plan);
    }
    stage::secrets(&mut plan);
    console::console(&mut plan);
    app::enterprise(&mut plan);
    if plan.stage.monitoring() {
        monitoring::monitoring(&mut plan);
    }
    plan.outputs
        .insert("StatWorkerUrl".into(), reference("Stat", "url"));
    if deploy_aws {
        plan.outputs
            .insert("StatsUrl".into(), reference("Stats", "url"));
    }
    plan.outputs
        .insert("AwsStage".into(), json!(plan.stage.aws_stage()));
    plan
}
