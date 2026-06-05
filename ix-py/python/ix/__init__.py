"""ix — sub-millisecond code search via sparse trigram indexing.

Module-level convenience functions:
    ix.search(pattern, path, **kwargs) -> SearchResult
    ix.build(path, **kwargs) -> dict
    ix.stats(path) -> dict
    ix.service_status(path) -> dict | None

Class:
    ix.Index(path) — opens a shard and exposes search/build/stats/close.
"""

from __future__ import annotations

import sys as _sys

try:
    from ._ix import (  # type: ignore[import-untyped]
        IxArchiveError,
        IxConfigError,
        IxCorruptionError,
        IxError,
        IxIndexError,
        IxIoError,
        IxRegexError,
        IxWatcherError,
        Index,
        Match,
        Pipeline,
        SearchResult,
    )
except ImportError as exc:
    raise ImportError(
        "ix native extension not available. "
        "Install a pre-built wheel for your platform:\n"
        f"    pip install ix\n"
        f"Platform: {_sys.platform}, "
        f"Python: {_sys.version_info.major}.{_sys.version_info.minor}\n"
        f"Original error: {exc}"
    ) from exc

__all__ = [
    "Index",
    "Match",
    "Pipeline",
    "SearchResult",
    "IxError",
    "IxIndexError",
    "IxCorruptionError",
    "IxIoError",
    "IxRegexError",
    "IxConfigError",
    "IxWatcherError",
    "IxArchiveError",
    "search",
    "build",
    "stats",
    "service_status",
]


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


def build(path: str, **kwargs: object) -> dict[str, object]:
    """Build or rebuild the index for the given path.

    Args:
        path: Root directory containing source files.
        **kwargs: Forwarded to `Index.build()`. Supports `max_file_size_mb`,
                  `exclude_dirs`.

    Returns:
        Dictionary with build statistics (6 fields).

    Raises:
        IxIoError: On I/O failure during build.
        IxConfigError: On configuration errors.
        NotImplementedError: If ix-py was built without the notify feature.
    """
    idx = Index(path)
    return idx.build(**kwargs)


def stats(path: str) -> dict[str, object]:
    """Return index-level statistics without performing a search.

    Args:
        path: Root directory or shard file path.

    Returns:
        Dictionary with file_count, trigram_count, and created_at.
    """
    idx = Index(path)
    return idx.stats()


def service_status(path: str) -> dict[str, object] | None:
    """Check whether ix daemon (ixd) is watching the given path.

    Args:
        path: Root directory to check for a running daemon.

    Returns:
        Dictionary with daemon beacon fields, or None if no daemon is running.

    Raises:
        NotImplementedError: If ix-py was built without the notify feature.
    """
    idx = Index(path)
    return idx.service_status()
