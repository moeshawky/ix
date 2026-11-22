"""Tests for module-level convenience functions — C-MODULE contract."""

from __future__ import annotations

from typing import TYPE_CHECKING

import ix

if TYPE_CHECKING:
    from pathlib import Path


def test_module_search(sample_project: Path) -> None:
    """ix.search creates Index, calls search, returns SearchResult."""
    result = ix.search("hello", str(sample_project))
    assert hasattr(result, "matches")
    assert hasattr(result, "stats")


def test_module_stats(sample_project: Path) -> None:
    """ix.stats creates Index, calls stats, returns all 11 IndexStats keys."""
    result = ix.stats(str(sample_project))
    assert isinstance(result, dict)
    for key in (
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
    ):
        assert key in result


def test_module_service_status(sample_project: Path) -> None:
    """ix.service_status returns None when no daemon runs or validated dict when stale/live."""
    result = ix.service_status(str(sample_project))
    assert result is None


def test_module_service_status_invalid_pid_ignored(sample_project: Path) -> None:
    """service_status treats non-positive or non-ixd beacon PID as not running (None)."""
    import json

    beacon_path = sample_project / ".ix" / "beacon.json"
    beacon_path.write_text(
        json.dumps(
            {
                "pid": 0,
                "root": str(sample_project),
                "start_time": 1,
                "status": "idle",
                "last_event_at": 1,
            }
        )
    )
    assert ix.service_status(str(sample_project)) is None


def test_module_all_public() -> None:
    """All promised functions and exceptions are in __all__."""
    for name in ("search", "build", "stats", "service_status", "LLMOSafeError"):
        assert name in ix.__all__


def test_module_build_creates_index(tmp_path: Path) -> None:
    """ix.build() creates a .ix directory with shard.ix and returns all 11 BuildStats keys."""
    (tmp_path / "hello.txt").write_text("hello world\n")
    result = ix.build(str(tmp_path))
    assert isinstance(result, dict)
    for key in (
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
    ):
        assert key in result

    index_path = tmp_path / ".ix" / "shard.ix"
    assert index_path.exists(), "ix.build() must create .ix/shard.ix"

    search_result = ix.search("hello", str(tmp_path))
    assert len(search_result.matches) > 0, "Index created by ix.build() should be searchable"
