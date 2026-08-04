//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // FIRST, before anything opens a database, a socket or a WebView. macOS
    // hands a GUI process a soft RLIMIT_NOFILE of 256; a meridian.db pool costs
    // three descriptors per connection (db/-wal/-shm) and the tray stacks a
    // WebView, sockets and in-process capture on top. Exhausting it fails a
    // -wal/-shm open mid-write with SQLITE_IOERR (522) and desyncs the shared
    // WAL index into "database disk image is malformed" (11). See
    // `meridian_core::fd_limit`.
    meridian_core::fd_limit::raise_fd_limit();
    meridian_tray_lib::run();
}
