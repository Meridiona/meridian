// meridian — normalises screenpipe activity into structured app sessions
//
// Browser-based OAuth 2.0 (Authorization Code + PKCE) for PM providers. Public
// desktop clients can't ship a client secret, so PKCE proves possession of the
// authorization code instead. Tokens expire and rotate, so the daemon owns a
// writable token store (`store`) and refreshes before use — unlike static API
// tokens which live read-only in `.env`.
//
//   pkce  — per-flow verifier/challenge/state generation (S256)
//   store — `~/.meridian/oauth/<provider>.json`, 0600, refresh-aware
//   flow  — generic loopback-redirect engine (browser → code → tokens → refresh)
//   jira  — Atlassian 3LO wiring: login, refresh-before-use, request-ctx resolver

pub mod flow;
pub mod jira;
pub mod pkce;
pub mod store;
