"""Tests for the error bridge — C-ERROR contract."""

from __future__ import annotations

from typing import TYPE_CHECKING

import ix

if TYPE_CHECKING:
    from pathlib import Path


def test_error_hierarchy() -> None:
    """IxError is the base for all ix exceptions in a 2-level hierarchy."""
    assert issubclass(ix.IxIndexError, ix.IxError)
    assert issubclass(ix.IxCorruptionError, ix.IxError)
    assert issubclass(ix.IxIoError, ix.IxError)
    assert issubclass(ix.IxRegexError, ix.IxError)
    assert issubclass(ix.IxConfigError, ix.IxError)
    assert issubclass(ix.IxWatcherError, ix.IxError)
    assert issubclass(ix.IxArchiveError, ix.IxError)
    assert issubclass(ix.LLMOSafeError, ix.IxError)


def test_all_errors_exported() -> None:
    """All 9 exception classes are in __all__."""
    for name in [
        "IxError",
        "IxIndexError",
        "IxCorruptionError",
        "IxIoError",
        "IxRegexError",
        "IxConfigError",
        "IxWatcherError",
        "IxArchiveError",
        "LLMOSafeError",
    ]:
        assert name in ix.__all__


def test_not_found_raises_index_error(empty_project: Path) -> None:
    """Opening a non-existent index raises IxIndexError."""
    import pytest

    with pytest.raises(ix.IxIndexError):
        ix.Index(str(empty_project))
