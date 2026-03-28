// ix — trigram code search library
//
// Build order: format → varint → trigram → bloom → posting →
//              string_pool → builder → reader → planner → executor → scanner

pub mod bloom;
pub mod builder;
pub mod config;
pub mod decompress;
pub mod archive;
pub mod error;
pub mod executor;
pub mod format;
#[cfg(feature = "notify")]
pub mod idle;
pub mod planner;
pub mod posting;
pub mod reader;
pub mod scanner;
pub mod string_pool;
pub mod trigram;
pub mod varint;
#[cfg(feature = "notify")]
pub mod watcher;
