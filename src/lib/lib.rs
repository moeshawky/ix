// ix — trigram code search library
//
// Build order: format → varint → trigram → bloom → posting →
//              string_pool → builder → reader → planner → executor → scanner

pub mod bloom;
pub mod builder;
pub mod config;
pub mod error;
pub mod executor;
pub mod format;
pub mod idle;
pub mod planner;
pub mod posting;
pub mod reader;
pub mod scanner;
pub mod string_pool;
pub mod trigram;
pub mod varint;
pub mod watcher;
