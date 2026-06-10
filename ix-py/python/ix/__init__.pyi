"""Type stubs for ix Python bindings."""

from __future__ import annotations

from typing import Any

__all__: list[str]

class Match:
    """A single regex match found in a file."""

    file_path: str
    line_number: int
    col: int
    line_content: str
    byte_offset: int
    context_before: list[str]
    context_after: list[str]
    is_binary: bool

    def __eq__(self, other: object) -> bool: ...
    def __hash__(self) -> int: ...
    def __repr__(self) -> str: ...

class SearchResult:
    """Container for search results — matches plus query statistics."""

    matches: list[Match]
    stats: dict[str, int]

class Index:
    """An open ix index backed by a memory-mapped shard file."""

    file_count: int
    trigram_count: int
    created_at: int
    path: str
    is_stale: bool

    def __init__(self, path: str, *, cache_mb: int = 64) -> None: ...
    def search(
        self,
        pattern: str,
        *,
        regex: bool = False,
        context_lines: int = 0,
        max_results: int = 0,
        type_filter: list[str] | None = None,
        multiline: bool = False,
        case_insensitive: bool = False,
        word_boundary: bool = False,
        count_only: bool = False,
        files_only: bool = False,
    ) -> SearchResult: ...
    @staticmethod
    def build(
        path: str,
        *,
        max_file_size_mb: int = 100,
        exclude_dirs: list[str] | None = None,
    ) -> dict[str, Any]: ...
    def rebuild(
        self,
        *,
        max_file_size_mb: int = 100,
        exclude_dirs: list[str] | None = None,
    ) -> dict[str, Any]: ...
    def stats(self) -> dict[str, Any]: ...
    def service_status(self) -> dict[str, Any] | None: ...
    def close(self) -> None: ...
    def __enter__(self) -> Index: ...
    def __exit__(
        self,
        exc_type: type | None,
        exc_value: BaseException | None,
        traceback: Any | None,
    ) -> None: ...
    def __repr__(self) -> str: ...

class IxError(Exception):
    """Root exception for all ix errors."""

class Pipeline:
    """LLMOSafe cognitive safety pipeline for Python consumers."""

    def __init__(self, objective: str) -> None: ...
    def process(self, text: str) -> dict: ...
    def get_decision(self) -> int: ...

class IxIndexError(IxError):
    """Index file cannot be opened or parsed."""

class IxCorruptionError(IxError):
    """Index data is internally corrupted."""

class IxIoError(IxError):
    """I/O or path errors."""

class IxRegexError(IxError):
    """Invalid regex pattern or matching failure."""

class IxConfigError(IxError):
    """Configuration-related errors."""

class IxWatcherError(IxError):
    """File-watcher errors."""

class IxArchiveError(IxError):
    """Archive-related errors."""

def search(pattern: str, path: str, **kwargs: Any) -> SearchResult: ...
def build(path: str, **kwargs: Any) -> dict[str, Any]: ...
def stats(path: str) -> dict[str, Any]: ...
def service_status(path: str) -> dict[str, Any] | None: ...
