const COMMANDS: &[&str] = &[
    "initialize",
    "set_client_authorization_header",
    "get_client_authorization_header",
];

fn main() {
    tauri_plugin::Builder::new(COMMANDS)
        .android_path("android")
        .ios_path("ios")
        .build();
}
