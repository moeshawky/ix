//! File-type → extension expansion, shared by the CLI and the daemon.
//!
//! The `--type` flag accepts high-level language names (`cpp`, `h`, `yaml`)
//! that each map to several concrete extensions. This module is the single
//! source of truth for that mapping: both the local search path
//! (`bin/ix` `do_search`) and the daemon (`daemon_sock::search`) call
//! [`expand`], so a given type resolves to the same extensions whether the
//! search runs locally or over IPC (audit F6 — the mapping previously lived
//! only in the CLI, so `--type cpp` over a live daemon matched only `.cpp`,
//! missing `.cc`/`.cxx`).

/// Expand high-level file-type names into their concrete file extensions.
///
/// `cpp` → `cpp`, `cc`, `cxx`; `h` → `h`, `hpp`; `yaml` → `yaml`, `yml`.
/// Every other name passes through unchanged (a name is treated as a literal
/// extension, so `rs` → `rs` and an arbitrary `myext` → `myext`).
pub fn expand(types: &[String]) -> Vec<String> {
    let mut extensions = Vec::new();
    for t in types {
        match t.as_str() {
            "cpp" => {
                extensions.push("cpp".to_string());
                extensions.push("cc".to_string());
                extensions.push("cxx".to_string());
            }
            "h" => {
                extensions.push("h".to_string());
                extensions.push("hpp".to_string());
            }
            "yaml" => {
                extensions.push("yaml".to_string());
                extensions.push("yml".to_string());
            }
            other => extensions.push(other.to_string()),
        }
    }
    extensions
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn test_expand_multi_extension_types() {
        assert_eq!(expand(&["cpp".to_string()]), vec!["cpp", "cc", "cxx"]);
        assert_eq!(expand(&["h".to_string()]), vec!["h", "hpp"]);
        assert_eq!(expand(&["yaml".to_string()]), vec!["yaml", "yml"]);
    }

    #[test]
    fn test_expand_single_and_passthrough() {
        assert_eq!(
            expand(&["rs".to_string(), "json".to_string()]),
            vec!["rs", "json"]
        );
        assert_eq!(expand(&["myext".to_string()]), vec!["myext"]);
        assert!(expand(&[]).is_empty());
    }
}
