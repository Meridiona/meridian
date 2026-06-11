//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    meridian_tray_lib::run();
}
