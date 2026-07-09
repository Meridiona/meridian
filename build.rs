//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

fn main() {
    // `option_env!("MERIDIAN_JIRA_OAUTH_CLIENT_SECRET")` in
    // src/intelligence/oauth/jira.rs is resolved at compile time. Cargo does NOT
    // track env vars as build inputs by default, so a CI cache (e.g. rust-cache)
    // could reuse a stale compilation and bake the OLD secret after a rotation
    // that changes only the build env. Declare the dependency so a changed secret
    // forces a recompile and the new value is picked up.
    println!("cargo::rerun-if-env-changed=MERIDIAN_JIRA_OAUTH_CLIENT_SECRET");
    // Same reasoning for the public GitHub OAuth device-flow client id, baked in
    // via option_env! in meridian-oauth/src/github.rs (compiled into this binary).
    println!("cargo::rerun-if-env-changed=MERIDIAN_GITHUB_OAUTH_CLIENT_ID");
}
