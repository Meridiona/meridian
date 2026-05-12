// meridian — normalises screenpipe activity into structured app sessions
// https://github.com/meridiona/meridian

pub mod extractor;
pub mod runner;
pub mod text_merge;

pub use runner::run_etl;
