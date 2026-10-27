# Hosted OAuth client acceptance

`oauth_tests.rs` exercises the composed console router over loopback TCP. A local
issuer fixture serves authorization redirects, discovery, JWKS and the token
endpoint, signing real ES256 JWTs. Redis must be a real isolated local fixture.
The test fails explicitly when `OPENCODE_P12_REDIS_URL` is missing or nonlocal.
It does not query MySQL, use provider credentials, or contact live services.

Check existing fixture setup and coordinate the shared p12 Cargo slot before
starting services or running builds. For an isolated disposable Redis container:

```sh
docker run --rm -d --name p12-oauth-fixture \
  -p 127.0.0.1::6379 redis:7-alpine redis-server --save '' --appendonly no
docker port p12-oauth-fixture 6379/tcp
```

Use the reported port in the Redis URL. From `rust/`:

```sh
export RUSTC_WRAPPER=sccache CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=4
export SCCACHE_BASEDIRS="$PWD/target" SCCACHE_CACHE_SIZE=100G
export CARGO_TARGET_DIR="$PWD/target/p12"
export OPENCODE_P12_REDIS_URL=redis://127.0.0.1:REPORTED_PORT/
cargo test -p opencode-hosted console::auth:: -- --nocapture
```

Stop only the fixture container you created, after any coordinated combined suite:

```sh
docker stop p12-oauth-fixture
```

Coverage includes both callback paths, redirect URI/form compatibility, sealed
cookie attributes and authenticated status, configured-origin handling, absent or
wrong session, altered cookie/state/signature, wrong JWT issuer/audience, expiry,
token exchange failure, replay after success or failure, and concurrent callback
consumption with exactly one successful token exchange.

This tests the console's OAuth client against a controlled external issuer.
It does not certify the Rust issuer runtime, provider browser login, account
invitation claiming, MySQL actor admission, or the deployed OpenAuth service.