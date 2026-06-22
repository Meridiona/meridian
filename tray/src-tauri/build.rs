//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
fn main() {
    // Compile-time runtime-channel pin: `mlx_server.rs` reads this via
    // `option_env!`, so a staging build (`npm run build:staging`) bakes the
    // `runtime-staging` manifest URL while production (`npm run build`) leaves it
    // unset and falls back to the `runtime-latest` default. rustc auto-tracks
    // `option_env!`, but declaring it here makes the env→binary dependency
    // explicit so switching channels locally never bakes a stale value.
    println!("cargo:rerun-if-env-changed=MERIDIAN_RUNTIME_MANIFEST_URL");
    tauri_build::build()
}
