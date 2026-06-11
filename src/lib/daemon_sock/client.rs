use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;

use super::resolve::socket_path;
use super::types::{
    ClientMessage, DaemonSockError, Result, SearchQuery, SearchResults, ServerMessage,
};

/// Client-side connection to an ixd daemon socket.
pub struct DaemonClient {
    /// Internal buffered Unix stream (visible for direct access in integration tests).
    pub(super) stream: BufReader<UnixStream>,
}

impl DaemonClient {
    /// Connect to the daemon socket for the given watched root.
    ///
    /// # Errors
    ///
    /// Returns an error if the socket does not exist or the connection fails.
    pub fn connect(root: &Path) -> Result<Self> {
        let sp = socket_path(root);
        let stream = UnixStream::connect(&sp)?;
        stream.set_read_timeout(Some(std::time::Duration::from_secs(5)))?;
        stream.set_write_timeout(Some(std::time::Duration::from_secs(5)))?;
        Ok(Self {
            stream: BufReader::new(stream),
        })
    }

    /// Receive the next message from the daemon (blocking).
    ///
    /// # Errors
    ///
    /// Returns an error on I/O failure, timeout (5s), or malformed JSON.
    pub fn recv(&mut self) -> Result<ServerMessage> {
        let mut line = String::new();
        let bytes = self.stream.read_line(&mut line).map_err(|e| {
            if e.kind() == std::io::ErrorKind::TimedOut {
                DaemonSockError::Io(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "recv timed out after 5s",
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
        let stream = self.stream.get_mut();
        let mut line = serde_json::to_string(msg)?;
        line.push('\n');
        stream.write_all(line.as_bytes())?;
        stream.flush()?;
        Ok(())
    }

    /// Execute a search query and return results.
    /// This sends a `SearchQuery` message and waits for the `SearchResults` response.
    ///
    /// # Errors
    ///
    /// Returns an error on I/O failure, timeout, if the response is not a
    /// `SearchResults`, or if the daemon reported a query execution error
    /// (e.g., invalid regex pattern) via the `SearchResults::error` field.
    pub fn search(&mut self, query: SearchQuery) -> Result<SearchResults> {
        use std::io::Write;

        let stream = self.stream.get_mut();
        stream.set_write_timeout(Some(std::time::Duration::from_secs(3)))?;

        // Send query
        let mut line = serde_json::to_string(&ClientMessage::SearchQuery(query))?;
        line.push('\n');
        stream.write_all(line.as_bytes())?;
        stream.flush()?;

        // Read response
        stream.set_read_timeout(Some(std::time::Duration::from_secs(3)))?;
        let mut response_line = String::new();
        self.stream.read_line(&mut response_line)?;

        match serde_json::from_str::<ServerMessage>(response_line.trim_end()) {
            Ok(ServerMessage::SearchResults(results)) => {
                // If the daemon encountered an error while executing the
                // query (e.g., invalid regex), propagate it to the caller
                // instead of treating it as a successful zero-match search.
                if let Some(ref error) = results.error {
                    return Err(DaemonSockError::Io(std::io::Error::other(format!(
                        "daemon search error: {error}"
                    ))));
                }
                Ok(results)
            }
            Ok(other) => Err(DaemonSockError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("expected SearchResults, got {other:?}"),
            ))),
            Err(e) => Err(DaemonSockError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("invalid JSON: {e}"),
            ))),
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
        use std::io::Write;

        let mut prog_query = query;
        prog_query.progressive = true;

        let stream = self.stream.get_mut();
        stream.set_write_timeout(Some(std::time::Duration::from_secs(3)))?;

        let mut line = serde_json::to_string(&ClientMessage::SearchQuery(prog_query))?;
        line.push('\n');
        stream.write_all(line.as_bytes())?;
        stream.flush()?;

        Ok(SearchResultsIter {
            client: self,
            done_received: false,
        })
    }
}

/// Iterator over progressive search result batches from the daemon.
///
/// Yields [`SearchResults`] messages until the daemon sends a batch with
/// `done: true`, at which point `next()` returns `None`.
///
/// Because the daemon keeps the socket open after sending the final batch
/// (so the client can send additional queries), the iterator tracks the
/// `done` flag and returns `None` without trying to read from the socket
/// after the final batch — avoiding a deadlock.
pub struct SearchResultsIter<'a> {
    client: &'a mut DaemonClient,
    /// Set to `true` after yielding a batch with `done: true`.
    /// The next `next()` call returns `None` without reading.
    done_received: bool,
}

impl Iterator for SearchResultsIter<'_> {
    type Item = Result<SearchResults>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done_received {
            return None;
        }
        match self.client.recv() {
            Ok(ServerMessage::SearchResults(results)) => {
                if let Some(ref error) = results.error {
                    return Some(Err(DaemonSockError::Io(std::io::Error::other(format!(
                        "daemon search error: {error}"
                    )))));
                }
                let is_done = results.done;
                if is_done {
                    self.done_received = true;
                }
                Some(Ok(results))
            }
            Ok(_) => Some(Err(DaemonSockError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "expected SearchResults message from daemon",
            )))),
            Err(e) => {
                if matches!(
                    &e,
                    DaemonSockError::Io(io_err) if io_err.kind() == std::io::ErrorKind::UnexpectedEof
                ) {
                    // Normal end of progressive stream.
                    None
                } else {
                    Some(Err(e))
                }
            }
        }
    }
}
