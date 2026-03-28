// ix — trigram code search library
//
// Build order: format → varint → trigram → bloom → posting →
//              string_pool → builder → reader → planner → executor → scanner

pub mod error;
pub mod format;
pub mod varint;
pub mod trigram;
pub mod bloom;
pub mod posting;
pub mod string_pool;
pub mod builder;
pub mod reader;
pub mod planner;
pub mod executor;
pub mod scanner;
pub mod idle;
pub mod watcher;
pub mod config;
