//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
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
