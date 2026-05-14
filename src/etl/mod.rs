// meridian — normalises screenpipe activity into structured app sessions
// https://github.com/meridiona/meridian

mod block_ops;
pub mod extractor;
pub mod runner;
mod session_builder;
pub mod text_merge;

pub use runner::run_etl;
