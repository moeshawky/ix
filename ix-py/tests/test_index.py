"""Tests for the Index class — C-INDEX contract."""

from __future__ import annotations

from pathlib import Path
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    import pytest

import ix


def test_stats(sample_project: Path) -> None:
    """Index.stats() returns file_count, trigram_count, created_at."""
    idx = ix.Index(str(sample_project))
    result = idx.stats()
    assert isinstance(result, dict)
    for key in ("file_count", "trigram_count", "created_at"):
        assert key in result
    assert isinstance(result["file_count"], int)
    assert isinstance(result["trigram_count"], int)


def test_properties(sample_project: Path) -> None:
    """Index properties expose header fields."""
    idx = ix.Index(str(sample_project))
    assert isinstance(idx.file_count, int)
    assert isinstance(idx.trigram_count, int)
    assert isinstance(idx.created_at, int)
    assert isinstance(idx.path, str)
    assert isinstance(idx.is_stale, bool)


def test_search_literal(sample_project: Path) -> None:
    """Literal search returns matches and stats."""
    idx = ix.Index(str(sample_project))
    result = idx.search("hello")
    assert hasattr(result, "matches")
    assert hasattr(result, "stats")
    assert isinstance(result.matches, list)
    assert isinstance(result.stats, dict)
    assert "total_matches" in result.stats


def test_search_regex(sample_project: Path) -> None:
    """Regex search works when regex=True."""
    idx = ix.Index(str(sample_project))
    result = idx.search(r"hello", regex=True)
    assert hasattr(result, "matches")
    assert hasattr(result, "stats")


def test_search_type_filter(sample_project: Path) -> None:
    """Type filter restricts results to matching extensions."""
    idx = ix.Index(str(sample_project))
    result = idx.search("hello", type_filter=["py"])
    for match in result.matches:
        assert match.file_path.endswith(".py")


def test_search_max_results(sample_project: Path) -> None:
    """max_results caps returned matches."""
    idx = ix.Index(str(sample_project))
    result = idx.search("hello", max_results=1)
    assert len(result.matches) <= 1


def test_search_context_lines(sample_project: Path) -> None:
    """Context lines are returned when requested."""
    idx = ix.Index(str(sample_project))
    result = idx.search("hello", context_lines=1)
    for match in result.matches:
        assert isinstance(match.context_before, list)
        assert isinstance(match.context_after, list)


def test_match_fields(sample_project: Path) -> None:
    """Match objects have all 8 fields."""
    idx = ix.Index(str(sample_project))
    result = idx.search("hello", max_results=1)
    if result.matches:
        m = result.matches[0]
        assert hasattr(m, "file_path")
        assert hasattr(m, "line_number")
        assert hasattr(m, "col")
        assert hasattr(m, "line_content")
        assert hasattr(m, "byte_offset")
        assert hasattr(m, "context_before")
        assert hasattr(m, "context_after")
        assert hasattr(m, "is_binary")
        assert isinstance(m.file_path, str)
        assert isinstance(m.line_number, int)
        assert isinstance(m.col, int)
        assert isinstance(m.line_content, str)
        assert isinstance(m.byte_offset, int)


def test_close_idempotent(sample_project: Path) -> None:
    """Close should not panic on repeated calls."""
    idx = ix.Index(str(sample_project))
    idx.close()
    idx.close()  # second call should be a no-op


def test_not_found_error(empty_project: str) -> None:
    """Non-existent path raises IxIndexError."""
    try:
        ix.Index(str(empty_project))
        assert False, "expected IxIndexError"
    except ix.IxIndexError:
        pass


def test_wrong_path_type_error() -> None:
    """Non-string or nonexistent path raises error."""
    try:
        ix.Index("/nonexistent/path/12345")
        assert False, "expected error"
    except (ix.IxIndexError, ix.IxIoError):
        pass


def test_build_no_notify_stub() -> None:
    """build() on index with notify may succeed or raise NotImplementedError."""
    import tempfile
    from pathlib import Path

    tmp = tempfile.mkdtemp()
    root = Path(tmp)
    (root / ".ix").mkdir()

    # Build using the CLI binary if available
    cargo_target = Path(__file__).parent.parent.parent / "target" / "debug" / "ix"
    if cargo_target.exists():
        import subprocess

        (root / "hello.py").write_text("print('hello')\n")
        subprocess.run(
            [str(cargo_target), "--build", str(root)],
            capture_output=True,
            timeout=30,
        )
        idx = ix.Index(str(root))
        try:
            result = idx.build()
            # If build succeeds, verify dict format
            assert "files_scanned" in result
        except NotImplementedError:
            pass  # ok if notify feature absent
