//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
fn main() {
    // Compile-time runtime-channel pin: `mlx_server.rs` reads this via
    // `option_env!`, so a staging build (`npm run build:staging`) bakes the
    // `runtime-staging` manifest URL while production (`npm run build`) leaves it
    // unset and falls back to the `runtime-latest` default. rustc auto-tracks
    // `option_env!`, but declaring it here makes the env→binary dependency
    // explicit so switching channels locally never bakes a stale value.
    println!("cargo:rerun-if-env-changed=MERIDIAN_RUNTIME_MANIFEST_URL");

    // Same compile-time pin, for the HF caching-proxy endpoint
    // (`mlx_server.rs::hf_endpoint`, `option_env!("MERIDIAN_HF_ENDPOINT")`).
    println!("cargo:rerun-if-env-changed=MERIDIAN_HF_ENDPOINT");

    // Same reasoning for the PostHog project API key (analytics.rs,
    // `option_env!("MERIDIAN_POSTHOG_API_KEY")`) — a GitHub Actions secret
    // baked into the release binary; see .github/workflows/release.yml.
    println!("cargo:rerun-if-env-changed=MERIDIAN_POSTHOG_API_KEY");

    // The notifications plugin's macOS layer is Swift (swift-bridge): its build
    // script links the static Swift lib but adds NO rpath, and the Swift
    // runtime dylibs it references (libswift_Concurrency & co.) resolve via
    // @rpath. Xcode-built binaries get /usr/lib/swift implicitly; a Rust link
    // does not — without this the packaged tray dies on launch with
    // `dyld: Library not loaded: @rpath/libswift_Concurrency.dylib (no
    // LC_RPATH's found)`. /usr/lib/swift is the OS-stable Swift runtime
    // (macOS 10.14.4+), so no dylibs need bundling.
    #[cfg(target_os = "macos")]
    println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");

    tauri_build::build()
}
