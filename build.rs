// meridian — normalises screenpipe activity into structured app sessions

fn main() {
    // `option_env!("MERIDIAN_JIRA_OAUTH_CLIENT_SECRET")` in
    // src/intelligence/oauth/jira.rs is resolved at compile time. Cargo does NOT
    // track env vars as build inputs by default, so a CI cache (e.g. rust-cache)
    // could reuse a stale compilation and bake the OLD secret after a rotation
    // that changes only the build env. Declare the dependency so a changed secret
    // forces a recompile and the new value is picked up.
    println!("cargo::rerun-if-env-changed=MERIDIAN_JIRA_OAUTH_CLIENT_SECRET");
}
