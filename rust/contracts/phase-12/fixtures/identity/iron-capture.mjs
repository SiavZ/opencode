// Fixture capture script (phase 12): seals {"a":1} with iron-webcrypto using fixed salts/IV so
// the Rust Fe26.2 implementation can be compared byte-for-byte. Run from packages/console/app
// with HOME/XDG pointed at a temp dir. Produces no side effects.
import * as iron from "../../../../../node_modules/.bun/iron-webcrypto@1.2.1/node_modules/iron-webcrypto/dist/index.js"

const pw = "fixture-session-secret-0123456789abcdef"
const salt1 = "01".repeat(32)
const salt2 = "02".repeat(32)
const iv = new Uint8Array(16).fill(3)
const enc = await iron.encrypt(crypto, pw, { ...iron.defaults.encryption, salt: salt1, iv }, '{"a":1}')
const base = `Fe26.2**${salt1}*${iron.base64urlEncode(iv)}*${iron.base64urlEncode(enc.encrypted)}*11000`
const mac = await iron.hmacWithPassword(crypto, pw, { ...iron.defaults.integrity, salt: salt2 }, base)
console.log(`${base}*${mac.salt}*${mac.digest}`)
