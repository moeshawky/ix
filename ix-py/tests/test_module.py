"""Tests for module-level convenience functions — C-MODULE contract."""

from __future__ import annotations

from pathlib import Path

import ix


def test_module_search(sample_project: Path) -> None:
    """ix.search creates Index, calls search, returns SearchResult."""
    result = ix.search("hello", str(sample_project))
    assert hasattr(result, "matches")
    assert hasattr(result, "stats")


def test_module_stats(sample_project: Path) -> None:
    """ix.stats creates Index, calls stats, returns dict."""
    result = ix.stats(str(sample_project))
    assert isinstance(result, dict)
    assert "file_count" in result
    assert "trigram_count" in result
    assert "created_at" in result


def test_module_service_status(sample_project: Path) -> None:
    """ix.service_status returns dict or None."""
    result = ix.service_status(str(sample_project))
    assert result is None or isinstance(result, dict)


def test_module_all_public() -> None:
    """All promised functions are in __all__."""
    for name in ("search", "build", "stats", "service_status"):
        assert name in ix.__all__


def test_module_build_creates_index(tmp_path: Path) -> None:
    """ix.build() creates a .ix directory with shard.ix in a fresh directory."""
    (tmp_path / "hello.txt").write_text("hello world\n")
    result = ix.build(str(tmp_path))
    assert isinstance(result, dict)

    index_path = tmp_path / ".ix" / "shard.ix"
    assert index_path.exists(), "ix.build() must create .ix/shard.ix"

    # Verify the index is functional by searching it
    search_result = ix.search("hello", str(tmp_path))
    assert len(search_result.matches) > 0, "Index created by ix.build() should be searchable"
