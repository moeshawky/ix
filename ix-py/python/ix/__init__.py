"""ix — sub-millisecond code search via sparse trigram indexing.

Module-level convenience functions:
    ix.search(pattern, path, **kwargs) -> SearchResult
    ix.build(path, **kwargs) -> BuildStats
    ix.stats(path) -> IndexStats
    ix.service_status(path) -> ServiceStatus | None

Class:
    ix.Index(path) — opens a shard and exposes search/build/stats/close.
"""

from __future__ import annotations

import sys as _sys
from typing import TypedDict, cast

try:
    from ._ix import (  # Native extension (PyO3/maturin)
        Index,
        IxArchiveError,
        IxConfigError,
        IxCorruptionError,
        IxError,
        IxIndexError,
        IxIoError,
        IxRegexError,
        IxWatcherError,
        LLMOSafeError,
        Match,
        Pipeline,
        SearchResult,
    )
except ImportError as exc:
    raise ImportError(
        "ix native extension not available. "
        "Install a pre-built wheel for your platform:\n"
        f"    pip install moeix\n"
        f"Platform: {_sys.platform}, "
        f"Python: {_sys.version_info.major}.{_sys.version_info.minor}\n"
        f"Original error: {exc}"
    ) from exc

__all__ = [
    "BuildStats",
    "Index",
    "IndexStats",
    "IxArchiveError",
    "IxConfigError",
    "IxCorruptionError",
    "IxError",
    "IxIndexError",
    "IxIoError",
    "IxRegexError",
    "IxWatcherError",
    "LLMOSafeError",
    "Match",
    "Pipeline",
    "SearchResult",
    "ServiceStatus",
    "build",
    "search",
    "service_status",
    "stats",
]


class IndexStats(TypedDict):
    """Index statistics returned by `stats()`.

    Attributes:
        shard_path: Path to the index shard file.
        file_count: Number of files in the index.
        trigram_count: Number of unique trigrams.
        source_bytes_total: Total source bytes indexed in the shard.
        created_at: Unix timestamp (microseconds) of index creation.
        version: Shard format version string (`major.minor`).
        has_cdx: Whether CDX block compression is enabled.
        has_bloom: Whether per-file bloom filters are present.
        has_content_hashes: Whether XXH64 content hashes are stored.
        posting_lists_compressed: Whether posting lists are ZSTD-compressed.
        posting_lists_checksummed: Whether posting lists carry CRC/XXHash checksums.
    """

    shard_path: str
    file_count: int
    trigram_count: int
    source_bytes_total: int
    created_at: int
    version: str
    has_cdx: bool
    has_bloom: bool
    has_content_hashes: bool
    posting_lists_compressed: bool
    posting_lists_checksummed: bool


class BuildStats(TypedDict):
    """Build statistics returned by `build()`.

    Attributes:
        path: Root directory path that was indexed.
        files_scanned: Number of files scanned during build.
        files_indexed: Number of files indexed in the shard.
        files_skipped: Total files skipped (binary + size-limited).
        files_skipped_binary: Number of binary files skipped.
        files_skipped_size: Number of oversized files skipped.
        bytes_scanned: Total bytes read across scanned files.
        unique_trigrams: Number of unique trigrams extracted.
        trigram_count: Alias for unique trigrams in the index.
        created_at: Unix timestamp (microseconds) of build completion.
        duration_ms: Build duration in milliseconds.
    """

    path: str
    files_scanned: int
    files_indexed: int
    files_skipped: int
    files_skipped_binary: int
    files_skipped_size: int
    bytes_scanned: int
    unique_trigrams: int
    trigram_count: int
    created_at: int
    duration_ms: int


class ServiceStatus(TypedDict):
    """Daemon service status returned by `service_status()`.

    Attributes:
        daemon_running: Whether the daemon is running.
        watching_path: Path being watched.
        index_path: Path to the index shard.
        is_stale: Whether the index needs rebuild.
        last_rebuild_at: Unix timestamp of last rebuild.
        pid: Process ID of the running daemon.
        root: Watched root path recorded in `beacon.json`.
        start_time: Unix timestamp when the daemon started.
        status: Current daemon state string.
        last_event_at: Unix timestamp of last filesystem event.
        socket_path: Path to daemon Unix socket (empty string if unset).
    """

    daemon_running: bool
    watching_path: str
    index_path: str
    is_stale: bool
    last_rebuild_at: int
    pid: int
    root: str
    start_time: int
    status: str
    last_event_at: int
    socket_path: str


def search(pattern: str, path: str, **kwargs: object) -> SearchResult:
    """Open an index from path and run a search, returning results immediately.

    Args:
        pattern: Non-empty search pattern string.
        path: Root directory or shard file path.
        **kwargs: Forwarded to `Index.search()`. Supports `regex`, `context_lines`,
                  `max_results`, `type_filter`, `multiline`, `case_insensitive`,
                  `word_boundary`, `count_only`, `files_only`.

    Returns:
        SearchResult with matches and query statistics.

    Raises:
        IxIndexError: If index is corrupt or not found.
        IxRegexError: If pattern is invalid regex.
    """
    with Index(path) as idx:
        return idx.search(pattern, **kwargs)


def build(path: str, **kwargs: object) -> BuildStats:
    """Build or rebuild the index for the given path.

    Args:
        path: Root directory containing source files.
        **kwargs: Forwarded to `Index.build()`. Supports `max_file_size_mb`,
                  `exclude_dirs`.

    Returns:
        BuildStats with all 11 build-report keys.

    Raises:
        IxIoError: On I/O failure during build.
        IxConfigError: On configuration errors.
        NotImplementedError: If ix-py was built without the notify feature.
    """
    result = Index.build(path, **kwargs)
    required = {
        "path",
        "files_scanned",
        "files_indexed",
        "files_skipped",
        "files_skipped_binary",
        "files_skipped_size",
        "bytes_scanned",
        "unique_trigrams",
        "trigram_count",
        "created_at",
        "duration_ms",
    }
    missing = required - set(result.keys())
    if missing:
        raise TypeError(
            f"Native extension returned incomplete BuildStats: missing {missing}"
        )
    return cast("BuildStats", result)


def stats(path: str) -> IndexStats:
    """Return index-level statistics without performing a search.

    Args:
        path: Root directory or shard file path.

    Returns:
        IndexStats with all 11 shard header fields.

    Raises:
        IxIndexError: If index is corrupt or not found.
    """
    with Index(path) as idx:
        result = idx.stats()
    required = {
        "shard_path",
        "file_count",
        "trigram_count",
        "source_bytes_total",
        "created_at",
        "version",
        "has_cdx",
        "has_bloom",
        "has_content_hashes",
        "posting_lists_compressed",
        "posting_lists_checksummed",
    }
    missing = required - set(result.keys())
    if missing:
        raise TypeError(
            f"Native extension returned incomplete IndexStats: missing {missing}"
        )
    return cast("IndexStats", result)


def service_status(path: str) -> ServiceStatus | None:
    """Check whether ix daemon (ixd) is watching the given path.

    Args:
        path: Root directory to check for a running daemon.

    Returns:
        ServiceStatus with all 11 status fields, or None if no daemon is running.

    Raises:
        NotImplementedError: If ix-py was built without the notify feature.
    """
    with Index(path) as idx:
        result = idx.service_status()
    if result is None:
        return None
    required = {
        "daemon_running",
        "watching_path",
        "index_path",
        "is_stale",
        "last_rebuild_at",
        "pid",
        "root",
        "start_time",
        "status",
        "last_event_at",
        "socket_path",
    }
    missing = required - set(result.keys())
    if missing:
        raise TypeError(
            f"Native extension returned incomplete ServiceStatus: missing {missing}"
        )
    return cast("ServiceStatus", result)
