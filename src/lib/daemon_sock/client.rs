use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

use super::resolve::socket_path;
use super::types::{
    ClientMessage, DaemonSockError, Result, SearchQuery, SearchResults, ServerMessage,
};

/// Maximum incoming IPC NDJSON line size accepted by the client (64 `MiB`).
const MAX_IPC_LINE_BYTES: usize = 64 * 1024 * 1024;

/// Maximum wall-clock time `DaemonClient::search` will spend skipping
/// unsolicited broadcast frames before timing out.
const SEARCH_BROADCAST_DEADLINE: Duration = Duration::from_secs(10);

struct TimeoutGuard {
    stream: UnixStream,
    prev_read: Option<Duration>,
    prev_write: Option<Duration>,
}

impl TimeoutGuard {
    fn new(stream: &UnixStream) -> std::io::Result<Self> {
        let cloned = stream.try_clone()?;
        let prev_read = cloned.read_timeout().ok().flatten();
        let prev_write = cloned.write_timeout().ok().flatten();
        Ok(Self {
            stream: cloned,
            prev_read,
            prev_write,
        })
    }
}

impl Drop for TimeoutGuard {
    fn drop(&mut self) {
        let _ = self.stream.set_read_timeout(self.prev_read);
        let _ = self.stream.set_write_timeout(self.prev_write);
    }
}

/// Client-side connection to an ixd daemon socket.
pub struct DaemonClient {
    /// Internal buffered Unix stream (visible for direct access in integration tests).
    pub(super) stream: BufReader<UnixStream>,
    /// Set to `true` after a search timeout or protocol framing error so
    /// subsequent requests cannot consume stale unread frames from the socket.
    pub(super) poisoned: bool,
}

impl DaemonClient {
    fn ensure_connected(&self) -> Result<()> {
        if self.poisoned {
            return Err(DaemonSockError::Io(std::io::Error::new(
                std::io::ErrorKind::ConnectionReset,
                "daemon client disconnected after timeout or protocol error; reconnect required",
            )));
        }
        Ok(())
    }

    fn disconnect_poisoned(&mut self) {
        self.poisoned = true;
        let _ = self.stream.get_mut().shutdown(std::net::Shutdown::Both);
    }
    /// Connect to the daemon socket for the given watched root.
    ///
    /// Resolves the socket path from the environment via [`socket_path`].
    ///
    /// # Errors
    ///
    /// Returns an error if the socket does not exist or the connection fails.
    pub fn connect(root: &Path) -> Result<Self> {
        Self::connect_with_socket(root, None)
    }

    /// Connect to the daemon socket, preferring the authoritative socket path
    /// recorded in the daemon's `beacon.json`.
    ///
    /// The daemon derives its socket path from its own canonical root and
    /// environment, which may differ from the client's (`$XDG_RUNTIME_DIR`,
    /// `$HOME`, or uid can differ between processes, and the root may
    /// canonicalize differently). Recomputing `socket_path(root)` client-side
    /// can therefore target a *different* socket than the one the daemon
    /// actually bound, leaving the daemon "up but silent". When
    /// `preferred_socket` is `Some`, it is used verbatim; otherwise the
    /// environment-derived [`socket_path`] is the fallback.
    ///
    /// # Errors
    ///
    /// Returns an error if the socket does not exist or the connection fails.
    pub fn connect_with_socket(root: &Path, preferred_socket: Option<&Path>) -> Result<Self> {
        let sp = match preferred_socket {
            Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
            _ => socket_path(root),
        };
        let stream = UnixStream::connect(&sp)?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;
        Ok(Self {
            stream: BufReader::new(stream),
            poisoned: false,
        })
    }

    /// Receive the next message from the daemon (blocking).
    ///
    /// Maps both `TimedOut` and `WouldBlock` socket read errors to `TimedOut`,
    /// rejects empty EOF reads with `UnexpectedEof`, and caps each incoming
    /// NDJSON frame to [`MAX_IPC_LINE_BYTES`].
    ///
    /// # Errors
    ///
    /// Returns an error on I/O failure, timeout, EOF, oversized frame, or
    /// malformed JSON.
    pub fn recv(&mut self) -> Result<ServerMessage> {
        self.ensure_connected()?;
        let mut buf = Vec::new();
        let limit = u64::try_from(MAX_IPC_LINE_BYTES)
            .unwrap_or(u64::MAX)
            .saturating_add(1);
        let bytes = (&mut self.stream)
            .take(limit)
            .read_until(b'\n', &mut buf)
            .map_err(|e| {
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                ) {
                    DaemonSockError::Io(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "recv timed out",
                    ))
                } else {
                    DaemonSockError::Io(e)
                }
            })?;
        if bytes == 0 {
            return Err(DaemonSockError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "daemon closed connection",
            )));
        }
        if buf.len() > MAX_IPC_LINE_BYTES {
            return Err(DaemonSockError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "IPC response line exceeded maximum allowed size",
            )));
        }
        let line = std::str::from_utf8(&buf).map_err(|e| {
            DaemonSockError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("invalid UTF-8 in IPC response: {e}"),
            ))
        })?;
        let msg: ServerMessage = serde_json::from_str(line.trim_end()).map_err(|e| {
            DaemonSockError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("invalid JSON: {e}"),
            ))
        })?;
        Ok(msg)
    }

    /// Send a query message to the daemon.
    ///
    /// # Errors
    ///
    /// Returns an error on I/O failure or serialization error.
    pub fn send(&mut self, msg: &ClientMessage) -> Result<()> {
        self.ensure_connected()?;
        let stream = self.stream.get_mut();
        let mut line = serde_json::to_string(msg)?;
        line.push('\n');
        stream.write_all(line.as_bytes())?;
        stream.flush()?;
        Ok(())
    }

    /// Execute a search query and return results.
    ///
    /// Sends a `SearchQuery` message and waits for the matching
    /// `SearchResults` response, skipping interleaved `Status`/`FilesChanged`
    /// broadcasts and stale `SearchResults` with non-matching IDs. Restores
    /// the socket's prior read and write timeouts before returning.
    ///
    /// # Errors
    ///
    /// Returns an error on I/O failure, timeout, daemon shutdown, if an
    /// unexpected response variant arrives, or if the daemon reported a query
    /// execution error via `SearchResults::error`.
    pub fn search(&mut self, query: SearchQuery) -> Result<SearchResults> {
        self.ensure_connected()?;
        let expected_id = query.id;
        let _timeout_guard = {
            let stream = self.stream.get_mut();
            let guard = TimeoutGuard::new(stream)?;
            stream.set_write_timeout(Some(Duration::from_secs(3)))?;
            stream.set_read_timeout(Some(Duration::from_secs(3)))?;
            let mut line = serde_json::to_string(&ClientMessage::SearchQuery(query))?;
            line.push('\n');
            if let Err(e) = stream
                .write_all(line.as_bytes())
                .and_then(|()| stream.flush())
            {
                drop(guard);
                self.disconnect_poisoned();
                return Err(e.into());
            }
            guard
        };

        let deadline = Instant::now() + SEARCH_BROADCAST_DEADLINE;
        loop {
            if Instant::now() >= deadline {
                self.disconnect_poisoned();
                return Err(DaemonSockError::Io(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "timed out waiting for matching SearchResults",
                )));
            }
            let msg = match self.recv() {
                Ok(m) => m,
                Err(e) => {
                    self.disconnect_poisoned();
                    return Err(e);
                }
            };
            match msg {
                ServerMessage::SearchResults(results) => {
                    if results.id != expected_id {
                        tracing::debug!(
                            "ignoring stale SearchResults id={} (expected {expected_id})",
                            results.id
                        );
                        continue;
                    }
                    // If the daemon encountered an error while executing the
                    // query (e.g., invalid regex), propagate it to the caller
                    // instead of treating it as a successful zero-match search.
                    if let Some(ref error) = results.error {
                        return Err(DaemonSockError::Io(std::io::Error::other(format!(
                            "daemon search error: {error}"
                        ))));
                    }
                    return Ok(results);
                }
                ServerMessage::Status { .. } | ServerMessage::FilesChanged { .. } => {}
                ServerMessage::Shutdown(notice) => {
                    self.disconnect_poisoned();
                    return Err(DaemonSockError::Io(std::io::Error::new(
                        std::io::ErrorKind::ConnectionAborted,
                        format!("daemon shutting down: {}", notice.reason),
                    )));
                }
                other @ ServerMessage::QueryResult { .. } => {
                    self.disconnect_poisoned();
                    return Err(DaemonSockError::Io(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("expected SearchResults, got {other:?}"),
                    )));
                }
            }
        }
    }

    /// Execute a search query progressively, returning an iterator over result batches.
    ///
    /// Sends a `SearchQuery` with `progressive: true` and returns an iterator
    /// that yields `SearchResults` batches as they arrive from the daemon.
    /// The iterator yields `None` when the daemon sends a batch with `done: true`.
    ///
    /// # Errors
    ///
    /// Returns an error on I/O failure or serialization error during the
    /// initial query send.
    pub fn search_progressive(&mut self, query: SearchQuery) -> Result<SearchResultsIter<'_>> {
        self.ensure_connected()?;
        let mut prog_query = query;
        prog_query.progressive = true;
        let expected_id = prog_query.id;

        {
            let stream = self.stream.get_mut();
            let timeout_guard = TimeoutGuard::new(stream)?;
            stream.set_write_timeout(Some(Duration::from_secs(3)))?;

            let mut line = serde_json::to_string(&ClientMessage::SearchQuery(prog_query))?;
            line.push('\n');
            if let Err(e) = stream
                .write_all(line.as_bytes())
                .and_then(|()| stream.flush())
            {
                drop(timeout_guard);
                self.disconnect_poisoned();
                return Err(e.into());
            }
            drop(timeout_guard);
        }

        Ok(SearchResultsIter {
            client: self,
            expected_id,
            done_received: false,
        })
    }
}

/// Iterator over progressive search result batches from the daemon.
///
/// Yields [`SearchResults`] messages matching the submitted query ID until the
/// daemon sends a batch with `done: true`, at which point `next()` returns
/// `None`. Unsolicited `Status`/`FilesChanged` broadcasts and stale
/// `SearchResults` from other query IDs are skipped transparently.
pub struct SearchResultsIter<'a> {
    client: &'a mut DaemonClient,
    expected_id: u64,
    /// Set to `true` after receiving a matching batch with `done: true`.
    /// The next `next()` call returns `None` without reading.
    done_received: bool,
}

impl Iterator for SearchResultsIter<'_> {
    type Item = Result<SearchResults>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done_received {
            return None;
        }
        loop {
            match self.client.recv() {
                Ok(ServerMessage::SearchResults(results)) => {
                    if results.id != self.expected_id {
                        continue;
                    }
                    if results.done {
                        self.done_received = true;
                    }
                    if let Some(ref error) = results.error {
                        return Some(Err(DaemonSockError::Io(std::io::Error::other(format!(
                            "daemon search error: {error}"
                        )))));
                    }
                    return Some(Ok(results));
                }
                Ok(ServerMessage::Status { .. } | ServerMessage::FilesChanged { .. }) => {}
                Ok(ServerMessage::Shutdown(notice)) => {
                    self.done_received = true;
                    return Some(Err(DaemonSockError::Io(std::io::Error::new(
                        std::io::ErrorKind::ConnectionAborted,
                        format!("daemon shutting down: {}", notice.reason),
                    ))));
                }
                Ok(other) => {
                    return Some(Err(DaemonSockError::Io(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("expected SearchResults message from daemon, got {other:?}"),
                    ))));
                }
                Err(e) => {
                    if matches!(
                        &e,
                        DaemonSockError::Io(io_err) if io_err.kind() == std::io::ErrorKind::UnexpectedEof
                    ) {
                        self.done_received = true;
                        self.client.disconnect_poisoned();
                        return Some(Err(e));
                    }
                    self.client.disconnect_poisoned();
                    return Some(Err(e));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon_sock::types::DaemonStatus;
    use crate::executor::QueryStats;

    fn sample_query(id: u64) -> SearchQuery {
        SearchQuery {
            id,
            pattern: "needle".into(),
            is_regex: false,
            ignore_case: false,
            word_boundary: false,
            max_results: 10,
            context_lines: 0,
            file_types: Vec::new(),
            decompress: false,
            multiline: false,
            archive: false,
            binary: false,
            search_path: None,
            progressive: false,
            chunk_size_bytes: 0,
            chunk_overlap_bytes: 0,
            count_only: false,
            files_only: false,
        }
    }

    fn write_msg(stream: &mut UnixStream, msg: &ServerMessage) {
        let mut line = serde_json::to_string(msg).expect("serialize");
        line.push('\n');
        stream.write_all(line.as_bytes()).expect("write");
        stream.flush().expect("flush");
    }

    #[test]
    fn search_skips_broadcasts_and_stale_ids_and_restores_timeout() {
        let (client_end, server_end) = UnixStream::pair().expect("pair");
        let initial_timeout = Duration::from_secs(7);
        client_end
            .set_read_timeout(Some(initial_timeout))
            .expect("set read timeout");
        client_end
            .set_write_timeout(Some(initial_timeout))
            .expect("set write timeout");

        let mut client = DaemonClient {
            stream: BufReader::new(client_end),
            poisoned: false,
        };

        let server_thread = std::thread::spawn(move || {
            let mut reader = BufReader::new(&server_end);
            let mut req = String::new();
            reader.read_line(&mut req).expect("read query");
            let mut writer = server_end.try_clone().expect("clone");

            // Send unsolicited broadcast + stale response + matching response.
            write_msg(
                &mut writer,
                &ServerMessage::Status {
                    pid: 1,
                    status: "idle".into(),
                    files: 3,
                    daemon_status: Some(DaemonStatus::Idle),
                },
            );
            write_msg(
                &mut writer,
                &ServerMessage::SearchResults(SearchResults {
                    id: 999,
                    matches: Vec::new(),
                    stats: QueryStats::default(),
                    error: None,
                    done: true,
                    batch: 0,
                }),
            );
            write_msg(
                &mut writer,
                &ServerMessage::SearchResults(SearchResults {
                    id: 42,
                    matches: Vec::new(),
                    stats: QueryStats {
                        total_matches: 5,
                        ..QueryStats::default()
                    },
                    error: None,
                    done: true,
                    batch: 0,
                }),
            );
        });

        let res = client.search(sample_query(42)).expect("search");
        assert_eq!(res.id, 42);
        assert_eq!(res.stats.total_matches, 5);
        assert_eq!(
            client
                .stream
                .get_ref()
                .read_timeout()
                .expect("read_timeout"),
            Some(initial_timeout)
        );
        assert_eq!(
            client
                .stream
                .get_ref()
                .write_timeout()
                .expect("write_timeout"),
            Some(initial_timeout)
        );
        server_thread.join().expect("server thread");
    }

    #[test]
    fn progressive_iter_marks_done_before_returning_error() {
        let (client_end, server_end) = UnixStream::pair().expect("pair");
        let mut client = DaemonClient {
            stream: BufReader::new(client_end),
            poisoned: false,
        };

        let server_thread = std::thread::spawn(move || {
            let mut reader = BufReader::new(&server_end);
            let mut req = String::new();
            reader.read_line(&mut req).expect("read query");
            let mut writer = server_end.try_clone().expect("clone");
            write_msg(
                &mut writer,
                &ServerMessage::SearchResults(SearchResults {
                    id: 11,
                    matches: Vec::new(),
                    stats: QueryStats::default(),
                    error: Some("bad regex".into()),
                    done: true,
                    batch: 0,
                }),
            );
        });

        let mut iter = client
            .search_progressive(sample_query(11))
            .expect("search_progressive");
        let first = iter.next().expect("first item");
        assert!(first.is_err());
        // Must return None immediately without blocking for a second batch.
        assert!(iter.next().is_none());
        server_thread.join().expect("server thread");
    }

    #[test]
    fn recv_maps_timeout_and_eof() {
        let (client_end, server_end) = UnixStream::pair().expect("pair");
        client_end
            .set_read_timeout(Some(Duration::from_millis(50)))
            .expect("set timeout");
        let mut client = DaemonClient {
            stream: BufReader::new(client_end),
            poisoned: false,
        };

        let err = client.recv().expect_err("must time out");
        assert!(matches!(
            err,
            DaemonSockError::Io(ref io_err) if io_err.kind() == std::io::ErrorKind::TimedOut
        ));

        drop(server_end);
        let eof = client.recv().expect_err("must hit EOF");
        assert!(matches!(
            eof,
            DaemonSockError::Io(ref io_err) if io_err.kind() == std::io::ErrorKind::UnexpectedEof
        ));
    }
}
