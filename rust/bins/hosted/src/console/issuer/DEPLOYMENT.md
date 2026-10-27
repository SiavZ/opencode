# Native console issuer

The native issuer runs separately from the console client on its own origin:

```text
OPENCODE_HOSTED_SERVICE=console-issuer
OPENCODE_HOSTED_ADDR=127.0.0.1:8788
OPENCODE_CONSOLE_DATABASE_URL=mysql://...
OPENCODE_STAGE=production
OPENCODE_ISSUER_URL=https://auth.opencode.ai
OPENCODE_ISSUER_STORE_PATH=/persistent/issuer.sqlite
OPENCODE_ISSUER_SIGNING_KEY_FILE=/secrets/issuer-es256.pk8.pem
OPENCODE_ISSUER_KEY_ID=stable-key-id
OPENCODE_ISSUER_GITHUB_CLIENT_ID=...
OPENCODE_ISSUER_GITHUB_CLIENT_SECRET=...
OPENCODE_ISSUER_GOOGLE_CLIENT_ID=...
```

Provision an ES256 P-256 PKCS8 PEM signing key outside the application. The application never silently generates ephemeral signing keys. Keep its key ID stable across restarts. Optional `OPENCODE_ISSUER_RETIRED_JWKS_FILE` points to a public-only `{ "keys": [...] }` document containing old ES256 or legacy RS512 public keys during rotation. Retain old public keys for at least the maximum remaining access-token lifetime, 30 days by default. Do not reuse a key ID for different key material. Provider credentials and signing key material are never included in configuration Debug output.

The SQLite file is a deployment-specific replacement for the baseline Cloudflare KV binding. It uses WAL, FULL synchronization, a five-second busy timeout and transactional one-time record consumption and refresh rotation. Put it on a persistent local filesystem that supports SQLite locking. Native-host file permissions are restricted to 0600 on Unix. Back up the database together with the active signing key metadata. Multiple hosts require a shared transactional storage implementation, not separate SQLite files or a network filesystem with unreliable locks. The public `IssuerStore` trait defines the required atomic operations for an alternative adapter.

Provider registrations must permit `https://auth.opencode.ai/github/callback` and `https://auth.opencode.ai/google/callback`, substituting the configured issuer origin. GitHub uses authorization code exchange with client secret, then authenticated `/user/emails` and `/user` retrieval. Google matches the installed OpenAuth baseline's implicit OIDC `response_type=id_token`, `response_mode=form_post`, client-ID-only flow. There is no Google token exchange requiring a Google client secret. Signatures, issuer, audience, expiry, issued-at, nonce, authorized-party and verified email are checked before account admission.

HTTPS browser cookies use the __Host- prefix, HttpOnly, Secure, SameSite=None so Google's cross-site form POST can carry the browser binding. Loopback HTTP is allowed for fixture servers. Place the native listener behind HTTPS on the configured public origin. Do not let untrusted clients control proxy routing or issuer configuration. The issuer URL is explicit and does not come from Host or forwarded headers.

The issuer exposes `/authorize`, `/github/authorize`, `/google/authorize`, provider callbacks, `/token`, `/userinfo`, `/.well-known/oauth-authorization-server` and `/.well-known/jwks.json`. Console mode does not expose these routes. Unknown providers and unsupported client-credentials grants cannot mint identities. Authorization requires client `app` and the existing console redirect allowlist. Accounts are issued only after the existing issuer_success policy resolves the verified identity, claims invited workspaces, verifies a subscribed nondeleted workspace and upserts identity links. Unknown and non-Black identities redirect to the new console without receiving credentials.

The public token claims and token response fields match installed OpenAuth: ES256 JWT with `kid`, `typ=JWT`, `mode=access`, `type=account`, `properties`, `aud`, `iss`, `sub`, `exp`; response fields `access_token`, `refresh_token`, `expires_in`. Codes expire after 60 seconds. Access tokens last 30 days; refresh grants last one year. Refresh reuse for 60 seconds returns the same preallocated successor. After that, the old refresh token expires. Default baseline retention is zero, so expiration does not revoke already issued successors. SQLite serialization prevents concurrent refreshes creating competing successors.

Intentional security tightening relative to the installed baseline: Google state and issuer checks, strict S256 PKCE parameter validation, HTTP status checks on GitHub identity endpoints, cookie-bound one-use provider flows, atomic authorization consumption across concurrent provider callbacks, redirect-disabled bounded HTTP clients, and no raw identity submission endpoint. Opaque server-side browser state replaces encrypted OpenAuth cookies; these internal cookies and stored records are not interchangeable during a rolling migration. Existing Cloudflare KV codes/refresh tokens require deliberate migration or reauthentication. Key files and store paths must be provisioned before startup.

Acceptance uses local fake provider HTTP services, real signature generation/verification, durable SQLite reopen and isolated MySQL baseline migrations. It does not prove production DNS, TLS termination, provider registrations, secret provisioning, Cloudflare KV record migration or multi-host storage operations. No deployment or live-provider call is performed by these tests.