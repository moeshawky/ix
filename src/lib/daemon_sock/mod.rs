//! Unix domain socket interface for the ixd daemon.
//!
//! Provides real-time file-change notifications and status queries over a
//! local Unix domain socket using NDJSON (newline-delimited JSON) framing.
//!
//! # Socket Path Resolution
//!
//! The socket path is derived from the canonical watched root:
//!
//! ```text
//! $XDG_RUNTIME_DIR/ixd/{hash}.sock        # preferred (systemd, modern Linux)
//! ~/.local/run/ixd/{hash}.sock             # fallback
//! /tmp/ixd-{uid}-{hash}.sock              # last resort
//! ```
//!
//! Where `hash` = first 16 hex chars of `XXH64(canonical_path, seed=0)`.
//!
//! # Wire Protocol (NDJSON)
//!
//! Each line is a valid JSON object terminated by `\\n`.
//!
//! **Server → Client (push):**
//!
//! ```json
//! {"t":"status","pid":1234,"status":"idle","files":1523}
//! {"t":"files_changed","batch":[{"p":"src/main.rs","m":1776468629,"o":"modify"}],"ts":1776468629}
//! ```
//!
//! **Client → Server (query):**
//!
//! ```json
//! {"t":"status_query"}
//! {"t":"history_query","since":1776468000,"id":1}
//! ```
//!
//! **Server → Client (query response):**
//!
//! ```json
//! {"t":"query_result","id":1,"status":"idle","files":1523,"changes_since":[...]}
//! ```

mod client;
mod resolve;
mod search;
mod server;
mod types;

// Complete re-export of all 15 public items.
pub use client::{DaemonClient, SearchResultsIter};
pub use resolve::socket_path;
pub(crate) use search::SearchCaches;
pub use search::{execute_search, execute_search_progressive};
pub use server::DaemonServer;
pub use types::{
    ClientMessage, DaemonSockError, DaemonStatus, FileChange, FileOp, SearchQuery, SearchResults,
    ServerMessage, ShutdownNotice,
};

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn socket_path_deterministic() {
        let root = PathBuf::from("/tmp/test-project");
        let p1 = socket_path(&root);
        let p2 = socket_path(&root);
        assert_eq!(p1, p2, "same root must produce same socket path");
    }

    #[test]
    fn socket_path_different_roots() {
        let r1 = PathBuf::from("/tmp/project-a");
        let r2 = PathBuf::from("/tmp/project-b");
        assert_ne!(socket_path(&r1), socket_path(&r2));
    }

    #[test]
    fn socket_path_uses_xdg() {
        unsafe { std::env::set_var("XDG_RUNTIME_DIR", "/tmp/xdg-test-runtime") };
        let p = socket_path(std::path::Path::new("/tmp/some-project"));
        assert!(p.starts_with("/tmp/xdg-test-runtime/ixd/"));
        assert!(p.extension().is_some_and(|e| e == "sock"));
        unsafe { std::env::remove_var("XDG_RUNTIME_DIR") };
    }

    #[test]
    fn client_connect_prefers_beacon_socket_path() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();

        // Pin the daemon's socket to a known env-derived location.
        unsafe { std::env::set_var("XDG_RUNTIME_DIR", "/tmp/ix-pref-sock-test") };
        let mut server = DaemonServer::new(&root).expect("create server");
        let authoritative = server.path().to_path_buf();
        let _ = server.start();

        // Same environment: the env-derived path matches the daemon's socket.
        let mut c1 = DaemonClient::connect(&root).expect("connect via env-derived path");
        c1.send(&ClientMessage::StatusQuery { id: 1 })
            .expect("send");

        // Drop XDG_RUNTIME_DIR so the env-derived path now resolves to a
        // DIFFERENT socket (one the daemon never bound). The authoritative
        // beacon socket path must still connect — this is the fix.
        unsafe { std::env::remove_var("XDG_RUNTIME_DIR") };
        let mut c2 = DaemonClient::connect_with_socket(&root, Some(&authoritative))
            .expect("connect via authoritative beacon socket path");
        c2.send(&ClientMessage::StatusQuery { id: 2 })
            .expect("send");
    }

    #[test]
    fn from_notify_kind_maps_rename_correctly() {
        use notify::EventKind;
        use notify::event::ModifyKind;

        // Rename events must map to FileOp::Rename, not Modify
        let kind = EventKind::Modify(ModifyKind::Name(notify::event::RenameMode::To));
        assert_eq!(FileOp::from_notify_kind(kind), FileOp::Rename);

        let kind = EventKind::Modify(ModifyKind::Name(notify::event::RenameMode::From));
        assert_eq!(FileOp::from_notify_kind(kind), FileOp::Rename);

        let kind = EventKind::Modify(ModifyKind::Name(notify::event::RenameMode::Both));
        assert_eq!(FileOp::from_notify_kind(kind), FileOp::Rename);

        // Non-name Modify events must map to Modify, not Rename
        let kind = EventKind::Modify(ModifyKind::Data(notify::event::DataChange::Content));
        assert_eq!(FileOp::from_notify_kind(kind), FileOp::Modify);

        // Create/Remove must NOT map to Rename
        let kind = EventKind::Create(notify::event::CreateKind::File);
        assert_eq!(FileOp::from_notify_kind(kind), FileOp::Create);

        let kind = EventKind::Remove(notify::event::RemoveKind::File);
        assert_eq!(FileOp::from_notify_kind(kind), FileOp::Delete);
    }

    #[test]
    fn server_message_ndjson_roundtrip() {
        let msg = ServerMessage::Status {
            pid: 1234,
            status: "idle".to_string(),
            files: 42,
            daemon_status: None,
        };
        let json = serde_json::to_string(&msg).expect("serialize");
        assert!(json.contains("\"t\":\"status\""), "tag field present");
        assert!(
            !json.contains("daemon_status"),
            "daemon_status should be omitted when None"
        );

        let back: ServerMessage = serde_json::from_str(&json).expect("deserialize");
        if let ServerMessage::Status {
            pid,
            status,
            files,
            daemon_status,
        } = back
        {
            assert_eq!(pid, 1234);
            assert_eq!(status, "idle");
            assert_eq!(files, 42);
            assert_eq!(daemon_status, None);
        } else {
            panic!("wrong variant after roundtrip");
        }
    }

    #[test]
    fn files_changed_roundtrip() {
        let msg = ServerMessage::FilesChanged {
            batch: vec![FileChange {
                path: PathBuf::from("src/main.rs"),
                mtime: 1_776_468_629,
                op: FileOp::Modify,
            }],
            timestamp: 1_776_468_629,
        };
        let json = serde_json::to_string(&msg).expect("serialize");
        let back: ServerMessage = serde_json::from_str(&json).expect("deserialize");
        if let ServerMessage::FilesChanged { batch, timestamp } = back {
            assert_eq!(batch.len(), 1);
            assert_eq!(batch[0].path, PathBuf::from("src/main.rs"));
            assert_eq!(timestamp, 1_776_468_629);
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn client_message_roundtrip() {
        let msg = ClientMessage::HistoryQuery { since: 1000, id: 7 };
        let json = serde_json::to_string(&msg).expect("serialize");
        let back: ClientMessage = serde_json::from_str(&json).expect("deserialize");
        if let ClientMessage::HistoryQuery { since, id } = back {
            assert_eq!(since, 1000);
            assert_eq!(id, 7);
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn server_client_connect_and_broadcast() {
        use std::io::BufReader;
        use std::os::unix::net::UnixStream;

        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();

        let mut server = DaemonServer::new(&root).expect("create server");
        let sp = server.path().to_path_buf();
        let _ = server.start();

        // Connect a client
        let stream = UnixStream::connect(&sp).expect("connect");
        let mut client = DaemonClient {
            stream: BufReader::new(stream),
        };

        // Give the accept thread time to register the client
        std::thread::sleep(std::time::Duration::from_millis(200));

        server.set_status(&DaemonStatus::Idle, 10);

        // Broadcast a status message
        server.broadcast(&ServerMessage::Status {
            pid: 1234,
            status: "idle".to_string(),
            files: 10,
            daemon_status: Some(DaemonStatus::Idle),
        });

        // Client should receive the message
        // Use a timeout to avoid hanging forever
        client
            .stream
            .get_mut()
            .set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .expect("set timeout");

        match client.recv() {
            Ok(ServerMessage::Status {
                pid,
                status,
                files,
                daemon_status,
            }) => {
                assert_eq!(pid, 1234);
                assert_eq!(status, "idle");
                assert_eq!(files, 10);
                assert!(daemon_status.is_some());
            }
            Ok(other) => panic!("expected Status, got {other:?}"),
            Err(e) => panic!("recv failed: {e}"),
        }
    }

    #[test]
    fn client_query_status() {
        use std::io::BufReader;
        use std::os::unix::net::UnixStream;

        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();

        let mut server = DaemonServer::new(&root).expect("create server");
        let sp = server.path().to_path_buf();
        let _ = server.start();
        server.set_status(&DaemonStatus::Indexing { entropy: 42 }, 99);

        let stream = UnixStream::connect(&sp).expect("connect");
        let mut client = DaemonClient {
            stream: BufReader::new(stream),
        };

        std::thread::sleep(std::time::Duration::from_millis(200));

        client
            .send(&ClientMessage::StatusQuery { id: 123 })
            .expect("send query");

        client
            .stream
            .get_mut()
            .set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .expect("set timeout");

        match client.recv() {
            Ok(ServerMessage::QueryResult {
                id,
                status,
                files,
                changes_since,
                daemon_status,
                last_rebuild_at,
            }) => {
                eprintln!(
                    "[JSON] id={id}, status={status}, files={files}, daemon_status={daemon_status:?}, last_rebuild_at={last_rebuild_at:?}"
                );
                assert_eq!(id, 123);
                assert_eq!(status, "indexing (entropy: 42)");
                assert_eq!(files, 99);
                assert!(changes_since.is_empty());
                assert_eq!(daemon_status, Some(DaemonStatus::Indexing { entropy: 42 }));
                assert_eq!(last_rebuild_at, None);
            }
            Ok(other) => panic!("expected QueryResult, got {other:?}"),
            Err(e) => panic!("recv failed: {e}"),
        }
    }

    #[test]
    fn search_query_defaults_search_path_none() {
        let json = r#"{"pattern":"hello"}"#;
        let q: SearchQuery = serde_json::from_str(json).expect("deserialize");
        assert!(q.search_path.is_none());
    }

    #[test]
    fn search_query_roundtrip_with_search_path() {
        let q = SearchQuery {
            id: 42,
            pattern: "findme".into(),
            is_regex: false,
            ignore_case: true,
            word_boundary: false,
            max_results: 10,
            context_lines: 2,
            file_types: vec!["rs".into()],
            decompress: false,
            multiline: false,
            archive: false,
            binary: false,
            search_path: Some(PathBuf::from("/abs/path")),
            progressive: false,
            chunk_size_bytes: 0,
            chunk_overlap_bytes: 0,
        };
        let json = serde_json::to_string(&q).expect("serialize");
        let back: SearchQuery = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.id, 42);
        assert_eq!(back.pattern, "findme");
        assert!(back.ignore_case);
        assert_eq!(back.max_results, 10);
        assert_eq!(back.context_lines, 2);
        assert_eq!(back.file_types, vec!["rs".to_string()]);
        assert_eq!(back.search_path, Some(PathBuf::from("/abs/path")));
    }

    #[test]
    fn search_query_omitting_search_path_is_backward_compatible() {
        let old_json = r#"{
            "pattern": "needle",
            "is_regex": true,
            "ignore_case": false,
            "word_boundary": true,
            "max_results": 0,
            "context_lines": 3,
            "file_types": [],
            "decompress": false,
            "multiline": true,
            "archive": false,
            "binary": false
        }"#;
        let q: SearchQuery = serde_json::from_str(old_json).expect("deserialize old client");
        assert_eq!(q.pattern, "needle");
        assert!(q.is_regex);
        assert!(q.multiline);
        assert_eq!(q.context_lines, 3);
        assert!(
            q.search_path.is_none(),
            "missing search_path in old client \u{2192} None"
        );
    }

    #[test]
    fn test_shutdown_protocol() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        let mut server = DaemonServer::new(&root).expect("create server");
        let _ = server.start();

        // Test shutdown notification
        server.shutdown_notify("test_signal", 100);

        // Give it time to broadcast
        std::thread::sleep(std::time::Duration::from_millis(150));

        // Server should still be functional after shutdown notify
        server.set_status(&DaemonStatus::Idle, 0);
    }

    #[test]
    fn test_client_shutdown_ack() {
        use std::io::BufReader;
        use std::os::unix::net::UnixStream;

        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        let mut server = DaemonServer::new(&root).expect("create server");
        let sp = server.path().to_path_buf();
        let _ = server.start();

        let stream = UnixStream::connect(&sp).expect("connect");
        let mut client = DaemonClient {
            stream: BufReader::new(stream),
        };

        // Client sends shutdown acknowledgment
        client
            .send(&ClientMessage::Shutdown { ack: true })
            .expect("send shutdown ack");

        // Give server time to process
        std::thread::sleep(std::time::Duration::from_millis(50));

        // Server should still be functional (shutdown ack is fire-and-forget)
        server.set_status(&DaemonStatus::Idle, 0);
    }
}
