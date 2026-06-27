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
from typing import TypedDict

try:
    from ._ix import (  # type: ignore[import-not-found]  # Native extension (PyO3/maturin)in)
        Index,
        IxArchiveError,
        IxConfigError,
        IxCorruptionError,
        IxError,
        IxIndexError,
        IxIoError,
        IxRegexError,
        IxWatcherError,
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
        file_count: Number of files in the index.
        trigram_count: Number of unique trigrams.
        created_at: Unix timestamp of index creation.
    """

    file_count: int
    trigram_count: int
    created_at: int


class BuildStats(TypedDict):
    """Build statistics returned by `build()`.

    Attributes:
        files_scanned: Number of files scanned during build.
        files_indexed: Number of files successfully indexed.
        files_skipped: Number of files skipped (size, binary, etc.).
        trigram_count: Number of unique trigrams in the index.
        created_at: Unix timestamp of build completion.
        duration_ms: Build duration in milliseconds.
    """

    files_scanned: int
    files_indexed: int
    files_skipped: int
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
    """

    daemon_running: bool
    watching_path: str
    index_path: str
    is_stale: bool
    last_rebuild_at: int


def search(pattern: str, path: str, **kwargs: object) -> SearchResult:
    """Open an index from path and run a search, returning results immediately.

    Args:
        pattern: Non-empty search pattern string.
        path: Root directory or shard file path.
        **kwargs: Forwarded to `Index.search()`. Supports `regex`, `context_lines`,
                  `max_results`, `type_filter`, `multiline`, `case_insensitive`,
                  `word_boundary`.

    Returns:
        SearchResult with matches and query statistics.

    Raises:
        IxIndexError: If index is corrupt or not found.
        IxRegexError: If pattern is invalid regex.
    """
    idx = Index(path)
    return idx.search(pattern, **kwargs)


def build(path: str, **kwargs: object) -> BuildStats:
    """Build or rebuild the index for the given path.

    Args:
        path: Root directory containing source files.
        **kwargs: Forwarded to `Index.build()`. Supports `max_file_size_mb`,
                  `exclude_dirs`.

    Returns:
        BuildStats with files_scanned, files_indexed, files_skipped,
        trigram_count, created_at, duration_ms.

    Raises:
        IxIoError: On I/O failure during build.
        IxConfigError: On configuration errors.
        NotImplementedError: If ix-py was built without the notify feature.
    """
    result = Index.build(path, **kwargs)
    # Runtime validation: ensure native extension returns expected fields
    required = {
        "files_scanned",
        "files_indexed",
        "files_skipped",
        "trigram_count",
        "created_at",
        "duration_ms",
    }
    missing = required - set(result.keys())
    if missing:
        raise TypeError(
            f"Native extension returned incomplete BuildStats: missing {missing}"
        )
    return result  # type: ignore[no-any-return]


def stats(path: str) -> IndexStats:
    """Return index-level statistics without performing a search.

    Args:
        path: Root directory or shard file path.

    Returns:
        IndexStats with file_count, trigram_count, and created_at.

    Raises:
        IxIndexError: If index is corrupt or not found.
    """
    result = Index(path).stats()
    # Runtime validation: ensure native extension returns expected fields
    required = {"file_count", "trigram_count", "created_at"}
    missing = required - set(result.keys())
    if missing:
        raise TypeError(
            f"Native extension returned incomplete IndexStats: missing {missing}"
        )
    return result  # type: ignore[no-any-return]


def service_status(path: str) -> ServiceStatus | None:
    """Check whether ix daemon (ixd) is watching the given path.

    Args:
        path: Root directory to check for a running daemon.

    Returns:
        ServiceStatus with daemon_running, watching_path, index_path,
        is_stale, last_rebuild_at; or None if no daemon is running.

    Raises:
        NotImplementedError: If ix-py was built without the notify feature.
    """
    result = Index(path).service_status()
    if result is None:
        return None
    # Runtime validation: ensure native extension returns expected fields
    required = {
        "daemon_running",
        "watching_path",
        "index_path",
        "is_stale",
        "last_rebuild_at",
    }
    missing = required - set(result.keys())
    if missing:
        raise TypeError(
            f"Native extension returned incomplete ServiceStatus: missing {missing}"
        )
    return result  # type: ignore[no-any-return]
