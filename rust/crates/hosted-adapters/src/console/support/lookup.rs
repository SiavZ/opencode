//! Purpose: query support workspace diagnostics by workspace id, key id, secret key or email.
//! Scope: read-only MySQL diagnostic repository matching the support application lookup.
//! Uses:
//! rust/crates/
//! |-- hosted-adapters/src/console/support.rs
//! `-- hosted-adapters/src/console/support/presentation.rs
//! External:
//! |-- mysql_async::{Conn, prelude::Queryable}
//! |-- opencode_hosted_domain::console::common::ConsoleError
//! `-- serde_json::{Value, json}
//! Used by:
//! rust/crates/
//! `-- hosted-adapters/src/console/support.rs
//! Invariants: all identifiers are bound parameters; key secrets are accepted but never returned.
//! Contract tests: hosted-adapters/tests/console_support.rs

use mysql_async::Conn;
use mysql_async::prelude::Queryable;
use opencode_hosted_domain::console::common::ConsoleError;
use serde_json::{Value, json};

use super::presentation::{dollars, go, user};
use super::{SupportResult, SupportStore};

async fn rows(conn: &mut Conn, sql: &str, id: &str) -> SupportResult<Vec<Value>> {
    let rows: Vec<String> = conn.exec(sql, (id,)).await?;
    rows.into_iter()
        .map(|row| {
            serde_json::from_str(&row).map_err(|_| {
                ConsoleError::Validation("Invalid support database record".into()).into()
            })
        })
        .collect()
}

impl SupportStore {
    /// `lookup.ts`: direct workspace/key lookup or every membership of the first matching account.
    /// Secret-key input is deliberately redacted in the returned identifier.
    pub async fn lookup(&self, identifier: &str, limits: &Value) -> SupportResult<Value> {
        self.lookup_at(identifier, limits, time::OffsetDateTime::now_utc())
            .await
    }

    /// Render all quota windows at one explicit instant, including across workspace memberships.
    pub async fn lookup_at(
        &self,
        identifier: &str,
        limits: &Value,
        now: time::OffsetDateTime,
    ) -> SupportResult<Value> {
        if identifier.is_empty() {
            return Err(ConsoleError::Validation("Identifier is required".into()).into());
        }
        if ![
            "rollingLimit",
            "rollingWindow",
            "weeklyLimit",
            "monthlyLimit",
        ]
        .iter()
        .all(|key| limits["lite"][key].as_i64().is_some())
        {
            return Err(
                ConsoleError::Validation("Support quota limits are not configured".into()).into(),
            );
        }
        let mut conn = self.db.conn().await?;
        let identifier_out = if identifier.starts_with("sk-") {
            "[redacted API key]"
        } else {
            identifier
        };
        if identifier.starts_with("wrk_")
            || identifier.starts_with("key_")
            || identifier.starts_with("sk-")
        {
            let id = if identifier.starts_with("wrk_") {
                identifier.to_string()
            } else {
                let query = if identifier.starts_with("key_") {
                    "SELECT workspace_id FROM `key` WHERE id = ? LIMIT 1"
                } else {
                    "SELECT workspace_id FROM `key` WHERE `key` = ? LIMIT 1"
                };
                conn.exec_first::<String, _, _>(query, (identifier,))
                    .await?
                    .ok_or_else(|| ConsoleError::NotFound("API key not found".into()))?
            };
            return Ok(
                json!({"identifier":identifier_out,"workspaces":[load_workspace(&mut conn,&id,limits,now).await?]}),
            );
        }
        let account: Option<String> = conn
            .exec_first(
                "SELECT account_id FROM auth WHERE subject = ? LIMIT 1",
                (identifier,),
            )
            .await?;
        let account = account.ok_or_else(|| ConsoleError::NotFound("Email not found".into()))?;
        let auth = rows(&mut conn,"SELECT JSON_OBJECT('provider',provider,'subject',subject,'accountID',account_id) FROM auth WHERE account_id = ?",&account).await?;
        let mut memberships = rows(&mut conn,"SELECT JSON_OBJECT('userID',u.id,'workspaceID',w.id,'name',w.name,'balance',b.balance,'role',u.role,'black',DATE_FORMAT(s.time_created,'%Y-%m-%d'),'lite',DATE_FORMAT(l.time_created,'%Y-%m-%d')) FROM `user` u RIGHT JOIN workspace w ON w.id = u.workspace_id LEFT JOIN billing b ON b.workspace_id = w.id LEFT JOIN subscription s ON s.user_id = u.id LEFT JOIN lite l ON l.user_id = u.id WHERE u.account_id = ?",&account).await?;
        let mut workspaces = Vec::new();
        for membership in &mut memberships {
            let id = membership["workspaceID"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let name = membership
                .as_object_mut()
                .unwrap()
                .remove("name")
                .unwrap_or(Value::Null);
            membership["workspaceName"] = json!({"__link":format!("#workspace-{id}"),"label":name});
            membership["balance"] =
                json!(dollars(membership["balance"].as_i64().unwrap_or_default()));
            if !id.is_empty() {
                workspaces.push(load_workspace(&mut conn, &id, limits, now).await?);
            }
        }
        Ok(
            json!({"identifier":identifier_out,"auth":auth,"accountWorkspaces":memberships,"workspaces":workspaces}),
        )
    }
}

async fn load_workspace(
    conn: &mut Conn,
    id: &str,
    limits: &Value,
    now: time::OffsetDateTime,
) -> SupportResult<Value> {
    let name: Option<String> = conn
        .exec_first("SELECT name FROM workspace WHERE id = ?", (id,))
        .await?;
    let name = name.ok_or_else(|| ConsoleError::NotFound(format!("Workspace {id} not found")))?;
    let users = rows(conn,"SELECT JSON_OBJECT('authEmail',a.subject,'inviteEmail',u.email,'role',u.role,'timeSeen',DATE_FORMAT(u.time_seen,'%Y-%m-%d'),'monthlyLimit',u.monthly_limit,'monthlyUsage',u.monthly_usage,'deleted',u.time_deleted IS NOT NULL,'fixedUsage',s.fixed_usage,'rollingUsage',s.rolling_usage,'timeFixedUpdated',UNIX_TIMESTAMP(s.time_fixed_updated),'timeRollingUpdated',UNIX_TIMESTAMP(s.time_rolling_updated),'subscribed',DATE_FORMAT(s.time_created,'%Y-%m-%d'),'subscription',b.subscription) FROM `user` u INNER JOIN billing b ON b.workspace_id = u.workspace_id LEFT JOIN auth a ON a.account_id = u.account_id AND a.provider = 'email' LEFT JOIN subscription s ON s.user_id = u.id WHERE u.workspace_id = ?",id).await?.into_iter().map(|row|user(row,limits,now)).collect::<Vec<_>>();
    let billing = rows(conn,"SELECT JSON_OBJECT('balance',balance,'customerID',customer_id,'reload',reload,'GO',lite_subscription_id,'subscriptionID',subscription_id,'subscription',subscription,'booked',time_subscription_booked IS NOT NULL,'plan',subscription_plan,'selected',time_subscription_selected IS NOT NULL) FROM billing WHERE workspace_id = ?",id).await?.into_iter().next().map(|row| {
        let mut billing = json!({"balance":dollars(row["balance"].as_i64().unwrap_or_default()),"reload":if truth(&row["reload"]) {"yes"} else {"no"},"customerID":row["customerID"],"GO":row["GO"]});
        if let Some(id) = row["subscriptionID"].as_str() {
            let subscription = &row["subscription"];
            let plan = subscription["plan"].as_str().unwrap_or_default();
            let seats = subscription["seats"].as_u64().unwrap_or(1);
            billing["Black"] = json!([format!("Black {plan}"),if seats>1 {format!("X {seats} seats")} else {String::new()},subscription["coupon"].as_str().map(|v|format!("(coupon: {v})")).unwrap_or_default(),format!("(ref: {id})")].join(" "));
        } else if truth(&row["booked"]) {
            billing["Black"] = json!(format!("Waitlist {} plan{}",row["plan"].as_str().unwrap_or_default(),if truth(&row["selected"]) {" (selected)"} else {""}));
        }
        billing
    });
    let go_rows = rows(conn,"SELECT JSON_OBJECT('userID',l.user_id,'userEmail',u.email,'authEmail',a.subject,'rollingUsage',l.rolling_usage,'weeklyUsage',l.weekly_usage,'monthlyUsage',l.monthly_usage,'timeRollingUpdated',UNIX_TIMESTAMP(l.time_rolling_updated),'timeWeeklyUpdated',UNIX_TIMESTAMP(l.time_weekly_updated),'timeMonthlyUpdated',UNIX_TIMESTAMP(l.time_monthly_updated),'timeCreated',UNIX_TIMESTAMP(l.time_created),'useBalance',b.lite) FROM lite l INNER JOIN billing b ON b.workspace_id = l.workspace_id LEFT JOIN `user` u ON u.id = l.user_id LEFT JOIN auth a ON a.account_id = u.account_id AND a.provider = 'email' WHERE l.workspace_id = ? AND l.time_deleted IS NULL",id).await?.into_iter().map(|row|go(row,limits,now)).collect::<Vec<_>>();
    let payments = rows(conn,"SELECT JSON_OBJECT('amount',amount,'paymentID',payment_id,'invoiceID',invoice_id,'customerID',customer_id,'timeCreated',DATE_FORMAT(time_created,'%Y-%m-%d'),'timeRefunded',DATE_FORMAT(time_refunded,'%Y-%m-%d')) FROM payment WHERE workspace_id = ? ORDER BY time_created DESC LIMIT 100",id).await?.into_iter().map(|mut row| {
        row["amount"] = json!(dollars(row["amount"].as_i64().unwrap_or_default()));
        if let Some(payment) = row["paymentID"].as_str() { row["paymentID"] = json!(format!("https://dashboard.stripe.com/acct_1RszBH2StuRr0lbX/payments/{payment}")); }
        row
    }).collect::<Vec<_>>();
    let usage = rows(conn,"SELECT JSON_OBJECT('date',CAST(day AS CHAR),'freeRequests',free_requests,'goRequests',go_requests,'goCost',go_cost,'apiRequests',api_requests,'apiCost',api_cost) FROM (SELECT DATE(time_created) AS day,SUM(CASE WHEN cost = 0 THEN 1 ELSE 0 END) AS free_requests,SUM(CASE WHEN JSON_UNQUOTE(JSON_EXTRACT(enrichment,'$.plan')) = 'lite' THEN 1 ELSE 0 END) AS go_requests,SUM(CASE WHEN JSON_UNQUOTE(JSON_EXTRACT(enrichment,'$.plan')) = 'lite' THEN cost ELSE 0 END) AS go_cost,SUM(CASE WHEN JSON_EXTRACT(enrichment,'$.plan') IS NULL AND cost > 0 THEN 1 ELSE 0 END) AS api_requests,SUM(CASE WHEN JSON_EXTRACT(enrichment,'$.plan') IS NULL AND cost > 0 THEN cost ELSE 0 END) AS api_cost FROM `usage` WHERE workspace_id = ? AND time_created >= DATE_SUB(NOW(), INTERVAL 28 DAY) GROUP BY DATE(time_created)) daily ORDER BY day DESC",id).await?;
    let mut total = json!({"date":"TOTAL","freeRequests":0,"goRequests":0,"goCost":0,"apiRequests":0,"apiCost":0});
    for row in &usage {
        for key in [
            "freeRequests",
            "goRequests",
            "goCost",
            "apiRequests",
            "apiCost",
        ] {
            total[key] = json!(
                total[key].as_i64().unwrap_or_default() + row[key].as_i64().unwrap_or_default()
            );
        }
    }
    let mut usage = usage;
    if !usage.is_empty() {
        usage.push(total);
    }
    for row in &mut usage {
        for key in ["goCost", "apiCost"] {
            row[key] = json!(dollars(row[key].as_i64().unwrap_or_default()));
        }
    }
    let disabled = rows(conn,"SELECT JSON_OBJECT('model',model,'timeCreated',DATE_FORMAT(time_created,'%Y-%m-%d')) FROM model WHERE workspace_id = ? ORDER BY time_created DESC",id).await?;
    Ok(
        json!({"workspaceID":id,"title":format!("Workspace \"{name}\" ({id})"),"users":users,"billing":billing,"go":go_rows,"payments":payments,"usage":usage,"disabledModels":disabled}),
    )
}

fn truth(value: &Value) -> bool {
    value
        .as_bool()
        .unwrap_or_else(|| value.as_i64().unwrap_or_default() != 0)
}
