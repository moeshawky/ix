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
