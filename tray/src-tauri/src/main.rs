// meridian — normalises screenpipe activity into structured app sessions
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    meridian_tray_lib::run();
}
