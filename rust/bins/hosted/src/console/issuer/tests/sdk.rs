//! Purpose: execute the installed baseline OpenAuth SDK against the native issuer HTTP service.
//! Scope: Bun child process with isolated HOME/XDG paths and loopback-only provider fixtures.
//! Uses:
//! rust/bins/
//! `-- hosted/src/console/issuer/tests.rs
//! External:
//! `-- std::process::Command
//! Used by:
//! rust/bins/
//! `-- hosted/src/console/issuer/tests.rs
//! Invariants: baseline packages and node_modules remain read-only; actual SDK performs verification.
//! Contract tests: parent public_issuer_claims_invites_checks_black_and_issues_verified_tokens

pub async fn acceptance(issuer: &str, account: &str, home: &std::path::Path) {
    let issuer = issuer.to_string();
    let account = account.to_string();
    let home = home.join(format!(
        "issuer-sdk-{}",
        time::OffsetDateTime::now_utc().unix_timestamp_nanos()
    ));
    std::fs::create_dir(&home).unwrap();
    let cleanup = home.clone();
    let output=tokio::task::spawn_blocking(move || {
        std::process::Command::new("bun")
            .current_dir(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../packages/console/function"))
            .env("HOME",&home).env("XDG_CONFIG_HOME",home.join("config")).env("XDG_DATA_HOME",home.join("data")).env("XDG_CACHE_HOME",home.join("cache"))
            .env("ISSUER_FIXTURE",issuer).env("ACCOUNT_FIXTURE",account)
            .arg("-e").arg(r#"
import { createClient } from '@openauthjs/openauth/client';
import { createSubjects } from '@openauthjs/openauth/subject';
import { z } from 'zod';
import assert from 'node:assert/strict';
const issuer=process.env.ISSUER_FIXTURE;
assert.equal(new URL(issuer).hostname,'127.0.0.1');
const subjects=createSubjects({account:z.object({accountID:z.string(),email:z.string(),newAccount:z.boolean().optional()})});
const client=createClient({clientID:'app',issuer,subjects});
const redirect='http://localhost:3456/auth/callback?code=stale&state=stale&keep=yes';
const start=await client.authorize(redirect,'code',{provider:'github',pkce:true});
const response=await fetch(start.url,{redirect:'manual'});
assert.equal(response.status,302);
const cookie=response.headers.get('set-cookie').split(';')[0];
const provider=new URL(response.headers.get('location'));
const callback=new URL('/github/callback',issuer);
callback.searchParams.set('state',provider.searchParams.get('state'));
callback.searchParams.set('code','github-fixture-code');
const denied=await fetch(callback,{redirect:'manual',headers:{cookie:cookie+'; '+cookie}});
assert.equal(denied.status,400);
const completed=await fetch(callback,{redirect:'manual',headers:{cookie}});
assert.equal(completed.status,302);
const location=new URL(completed.headers.get('location'));
assert.equal(location.searchParams.getAll('code').length,1);
assert.equal(location.searchParams.getAll('state').length,1);
assert.equal(location.searchParams.get('state'),start.challenge.state);
assert.equal(location.searchParams.get('keep'),'yes');
const exchanged=await client.exchange(location.searchParams.get('code'),redirect,start.challenge.verifier);
assert.equal(exchanged.err,false);
const verified=await client.verify(exchanged.tokens.access);
assert.ok(!verified.err);
assert.equal(verified.aud,'app');
assert.equal(verified.subject.id,process.env.ACCOUNT_FIXTURE);
assert.equal(verified.subject.type,'account');
assert.deepEqual(verified.subject.properties,{accountID:process.env.ACCOUNT_FIXTURE,email:'issuer@anoma.ly',newAccount:false});
const refreshed=await client.refresh(exchanged.tokens.refresh);
assert.equal(refreshed.err,false);
const refreshVerified=await client.verify(refreshed.tokens.access);
assert.ok(!refreshVerified.err);
assert.equal(refreshVerified.subject.id,process.env.ACCOUNT_FIXTURE);
const replay=await client.exchange(location.searchParams.get('code'),redirect,start.challenge.verifier);
assert.ok(replay.err);
const deniedStart=await client.authorize(redirect,'code',{provider:'google'});
const deniedResponse=await fetch(deniedStart.url,{redirect:'manual'});
const deniedCookie=deniedResponse.headers.get('set-cookie').split(';')[0];
const deniedProvider=new URL(deniedResponse.headers.get('location'));
const failure=await fetch(new URL('/google/callback',issuer),{method:'POST',redirect:'manual',headers:{cookie:deniedCookie,'Content-Type':'application/x-www-form-urlencoded'},body:new URLSearchParams({state:deniedProvider.searchParams.get('state'),error:'access_denied',error_description:'User declined'})});
assert.equal(failure.status,302);
const errorLocation=new URL(failure.headers.get('location'));
assert.equal(errorLocation.origin,'http://localhost:3456');
assert.equal(errorLocation.searchParams.get('error'),'access_denied');
assert.equal(errorLocation.searchParams.get('error_description'),'User declined');
console.log('Installed OpenAuth SDK authorize/exchange/verify/refresh/replay/denial acceptance passed');
"#).output().expect("installed Bun required for SDK compatibility acceptance")
    }).await.unwrap();
    std::fs::remove_dir_all(cleanup).unwrap();
    assert!(
        output.status.success(),
        "SDK fixture failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
