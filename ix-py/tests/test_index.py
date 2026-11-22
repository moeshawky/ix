"""Tests for the Index class — C-INDEX contract."""

from __future__ import annotations

from typing import TYPE_CHECKING

import ix
import pytest

if TYPE_CHECKING:
    from pathlib import Path


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
    with pytest.raises(ix.IxIndexError):
        ix.Index(str(empty_project))


def test_wrong_path_type_error() -> None:
    """Non-string or nonexistent path raises error."""
    with pytest.raises((ix.IxIndexError, ix.IxIoError)):
        ix.Index("/nonexistent/path/12345")


def test_build_no_notify_stub(tmp_path: Path) -> None:
    """rebuild(), build(), and service_status() work with notify or raise NotImplementedError."""
    import os
    import pathlib
    import subprocess

    expect_no_notify = os.environ.get("IX_EXPECT_NO_NOTIFY") == "1"
    (tmp_path / ".ix").mkdir()
    (tmp_path / "hello.py").write_text("print('hello')\n")

    repo_root = pathlib.Path(__file__).parent.parent.parent
    cargo_target = repo_root / "target" / "debug" / "ix"
    if not cargo_target.exists():
        cargo_target = repo_root / "target" / "release" / "ix"

    built = False
    if cargo_target.exists():
        subprocess.run(
            [str(cargo_target), "--build", str(tmp_path)],
            capture_output=True,
            timeout=30,
            check=True,
        )
        built = True
    elif not expect_no_notify:
        ix.build(str(tmp_path))
        built = True

    assert built, (
        "ix CLI binary required to pre-build index under not(feature = 'notify')"
    )

    with ix.Index(str(tmp_path)) as idx:
        if expect_no_notify:
            with pytest.raises(NotImplementedError):
                idx.rebuild()
            with pytest.raises(NotImplementedError):
                ix.Index.build(str(tmp_path))
            with pytest.raises(NotImplementedError):
                ix.build(str(tmp_path))
            with pytest.raises(NotImplementedError):
                idx.service_status()
            return

        try:
            result = idx.rebuild()
            assert "files_scanned" in result
        except NotImplementedError:
            with pytest.raises(NotImplementedError):
                ix.Index.build(str(tmp_path))
            with pytest.raises(NotImplementedError):
                idx.service_status()


def test_context_manager_protocol(sample_project: Path) -> None:
    """Index supports the context manager protocol (__enter__/__exit__)."""
    import pytest

    with ix.Index(str(sample_project)) as idx:
        result = idx.search("hello")
        assert hasattr(result, "matches")
        assert len(result.matches) > 0
    with pytest.raises(RuntimeError):
        idx.search("hello")


def test_close_blocks_all_methods_and_properties(sample_project: Path) -> None:
    """close() prevents subsequent stats, rebuild, service_status, and property reads."""
    import pytest

    idx = ix.Index(str(sample_project))
    idx.close()
    for call in (
        lambda: idx.search("hello"),
        idx.stats,
        idx.rebuild,
        idx.service_status,
        lambda: idx.file_count,
        lambda: idx.trigram_count,
        lambda: idx.created_at,
        lambda: idx.is_stale,
    ):
        with pytest.raises(RuntimeError, match="closed"):
            call()


def test_match_equality_and_stats_fields(sample_project: Path) -> None:
    """Match == non-Match returns False and QueryStats includes all 14 fields."""
    with ix.Index(str(sample_project)) as idx:
        result = idx.search("hello", context_lines=1)
        assert len(result.matches) > 0
        m = result.matches[0]
        assert (m == None) is False  # noqa: E711
        assert (m == "x") is False
        assert m == result.matches[0]

        for key in (
            "trigrams_queried",
            "posting_lists_decoded",
            "candidate_files",
            "files_verified",
            "files_failed_verify",
            "files_skipped_pressure",
            "bytes_verified",
            "lines_read",
            "total_matches",
            "total_available",
            "posting_cache_hits",
            "posting_cache_misses",
            "neg_cache_hits",
            "neg_cache_misses",
        ):
            assert key in result.stats


def test_capacity_overflow_raises_value_error(sample_project: Path) -> None:
    """Overflowing cache_mb or max_file_size_mb raises ValueError instead of wrapping."""
    import pytest

    huge_mb = 1 << 60
    with pytest.raises(ValueError, match="overflow"):
        ix.Index(str(sample_project), cache_mb=huge_mb)
    with pytest.raises(ValueError, match="overflow"):
        ix.Index.build(str(sample_project), max_file_size_mb=huge_mb)


def test_type_filter_expands_aliases_and_reload_on_rebuild(tmp_path: Path) -> None:
    """type_filter=['cpp'] expands aliases and stale mmaps reload automatically."""
    (tmp_path / "widget.cc").write_text("int cpp_anchor_alpha() { return 1; }\n")
    ix.build(str(tmp_path))

    with ix.Index(str(tmp_path)) as idx:
        res = idx.search("cpp_anchor_alpha", type_filter=["cpp"])
        assert len(res.matches) == 1
        assert res.matches[0].file_path.endswith(".cc")

        (tmp_path / "extra.cxx").write_text("int cpp_anchor_beta() { return 2; }\n")
        ix.build(str(tmp_path))
        res_after = idx.search("cpp_anchor_beta", type_filter=["cpp"])
        assert len(res_after.matches) == 1
        assert res_after.matches[0].file_path.endswith(".cxx")
