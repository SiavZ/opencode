# Native Zen gateway error localization

`translate(message: &str, headers: &http::HeaderMap, path: &str) -> String`
localizes known English gateway error templates. Only native gateway errors
should call it. Provider responses and the separate Go usage handler remain
untouched, matching the TypeScript producer boundaries.

`dictionaries.json` contains all 21 `zen.api.error.*` keys from each of the 18
actual `packages/console/app/src/i18n/*.ts` dictionaries. Captured locally on
2026-10-27 without editing the baseline. Missing localized keys fall back to
English, as `i18n/index.ts` does.

`fixtures.json` contains 398 expected cases:

- 18 languages times 21 error templates, including model/provider strings,
  numeric monetary limits, retry text, billing/member/Go links and `$&` literals.
- 20 request-locale cases executed through actual
  `packages/console/app/src/lib/language.ts` `localeFromRequest`.

Interpolation uses the same placeholder replacement semantics as handler.ts's
`resolve`. Rust captures placeholders from an anchored English template and
renders the localized template using callback replacement. Captured `$`, braces,
backslashes, URL punctuation and user-visible model strings remain literal.
Unknown messages are returned unchanged. This intentionally does not translate
arbitrary text resembling an upstream error object.

Locale selection follows the baseline ordering:

1. Strict case-sensitive `x-opencode-locale` value.
2. Strict locale in the first pathname segment.
3. Case-insensitive docs locale alias in `/docs/<alias>/...`.
4. First `oc_locale` cookie, percent-decoded with strict locale matching.
5. Stable descending Accept-Language weights, with the baseline prefix/Chinese
   script/Norwegian/Brazilian-Portuguese mappings and English fallback.

One documented hardening difference: malformed percent encoding in the cookie
falls through to Accept-Language. The baseline's `decodeURIComponent` throws;
a malformed cookie must not panic a native request. Missing or invalid ordinary
locale values follow baseline fallback behavior.

Unit tests are in `tests.rs`. Run from `rust/` with the phase coordinator's
exclusive Cargo slot and mandatory p12 target/sccache environment:

```sh
cargo test -p opencode-hosted console::zen::locale::tests
```

These are offline fixture tests. Gateway HTTP boundary acceptance belongs to the
gateway owner's suite.