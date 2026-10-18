//! Purpose: port of infra/stats.ts (AWS VPC/cluster, stats PlanetScale DB, stats site, R2 SQL
//! linkables and the singleton ARM64 sync service). Only built when the stage deploys AWS.
//! Scope: pure plan construction.
//! Uses:
//! rust/xtask/
//! |-- src/cloud/infra/console.rs
//! |-- src/cloud/infra/stage.rs
//! `-- src/cloud/model.rs
//! External:
//! `-- serde_json::json
//! Used by:
//! rust/xtask/
//! `-- src/cloud/infra/mod.rs
//! Invariants: scaling min=max=1 (singleton), arm64, 0.25 vCPU, 2 GB (infra/stats.ts:104-126).
//! R2 SQL bucket is `platform-<stage>-lake` (infra/stats.ts:98).
//! Contract tests: xtask/tests/cloud_plan.rs

use serde_json::json;

use crate::cloud::infra::console::planetscale;
use crate::cloud::infra::stage::secret;
use crate::cloud::model::{Mode, Plan, Resource, reference};

pub fn stats(plan: &mut Plan) {
    let domain = plan.stage.domain();
    plan.add(Resource::new(
        Mode::Managed,
        "sst.aws.Vpc",
        "LakeVpc",
        "infra/stats.ts:6",
    ));
    plan.add(
        Resource::new(
            Mode::Managed,
            "sst.aws.Cluster",
            "LakeCluster",
            "infra/stats.ts:7",
        )
        .props(json!({ "vpc": reference("LakeVpc", "id") })),
    );
    planetscale(
        plan,
        "Stats",
        "opencode-stats",
        [
            "infra/stats.ts:13-16",
            "infra/stats.ts:18-30",
            "infra/stats.ts:32-37",
            "infra/stats.ts:39-52",
        ],
        true,
    );
    plan.add(
        Resource::new(
            Mode::DevCommand,
            "sst.x.DevCommand",
            "StatsStudio",
            "infra/stats.ts:54-64",
        )
        .props(json!({
            "environment": { "DATABASE_URL": reference("StatsDatabase", "url") },
            "command": "bun db:studio", "directory": "packages/stats/core", "autostart": false
        }))
        .links(["StatsDatabase"]),
    );
    plan.add(
        Resource::new(
            Mode::Managed,
            "sst.cloudflare.x.SolidStart",
            "Stats",
            "infra/stats.ts:70-78",
        )
        .props(json!({
            "path": "packages/stats/app",
            "build_command": "bun run build",
            "domain": format!("stats.{domain}"),
            "environment": { "PUBLIC_URL": format!("https://{domain}/data") }
        }))
        .links(["StatsDatabase", "EMAILOCTOPUS_API_KEY"]),
    );
    plan.add(
        Resource::new(
            Mode::Linkable,
            "sst.Linkable",
            "StatsSyncConfig",
            "infra/stats.ts:84-88",
        )
        .props(json!({ "dataset": "zen" })),
    );
    plan.add(secret("StatsHiddenModels", "infra/stats.ts:92", true));
    plan.add(secret("R2SqlAuthToken", "infra/stats.ts:94", false));
    plan.add(
        Resource::new(
            Mode::Linkable,
            "sst.Linkable",
            "R2Sql",
            "infra/stats.ts:95-102",
        )
        .props(json!({
            "accountId": "15d29c8639fd3733b1b5486a2acfd968",
            "bucket": format!("platform-{}-lake", plan.stage.name),
            "namespace": "inference",
            "table": "generation"
        })),
    );
    plan.add(
        Resource::new(Mode::Managed, "sst.aws.Service", "StatsSyncService", "infra/stats.ts:104-126")
            .props(json!({
                "cluster": reference("LakeCluster", "id"),
                "architecture": "arm64",
                "cpu": "0.25 vCPU",
                "memory": "2 GB",
                "image": { "context": ".", "dockerfile": "packages/stats/server/Dockerfile" },
                "command": ["bun", "src/stat-sync.ts"],
                "scaling": { "min": 1, "max": 1 },
                "dev": { "command": "bun src/stat-sync.ts", "directory": "packages/stats/server", "autostart": false }
            }))
            .links(["StatsDatabase", "R2Sql", "R2SqlAuthToken", "StatsHiddenModels", "StatsSyncConfig"]),
    );
}
