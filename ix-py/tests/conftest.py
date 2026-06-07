"""Shared fixtures for ix Python binding tests."""

from __future__ import annotations

import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any, Generator

import pytest


def _build_ix_py() -> None:
    """Build the ix-py extension module in development mode."""
    # Reuse the existing target directory from the workspace
    result = subprocess.run(
        [
            sys.executable,
            "-m",
            "pip",
            "install",
            "--no-build-isolation",
            "-e",
            f"{Path(__file__).parent.parent}",
        ],
        capture_output=True,
        text=True,
        env={**os.environ, "MATURIN_PEP517_ARGS": "--interpreter python3"},
    )
    if result.returncode != 0:
        print(f"STDERR: {result.stderr}")
        print(f"STDOUT: {result.stdout}")


@pytest.fixture(scope="session")
def ix_module() -> Any:
    """Ensure ix._ix is built and importable."""
    _build_ix_py()
    import ix  # noqa: F401
    import ix._ix  # type: ignore[import-not-found]

    return ix._ix


@pytest.fixture
def sample_project() -> Generator[Path, None, None]:
    """Create a temporary project with source files and a pre-built .ix index."""
    tmp = tempfile.mkdtemp()
    root = Path(tmp)
    (root / ".ix").mkdir()

    # Write source files
    (root / "main.py").write_text("def hello():\n    print('hello world')\n")
    (root / "lib.rs").write_text("fn hello() -> &'static str {\n    \"hello world\"\n}\n")
    (root / "src").mkdir()
    (root / "src" / "app.js").write_text(
        "function hello() {\n    console.log('hello');\n}\n"
    )

    # Build index using the ix CLI binary
    binary_name = "ix.exe" if sys.platform == "win32" else "ix"
    cargo_target = Path(__file__).parent.parent.parent / "target" / "debug" / binary_name
    if not cargo_target.exists():
        cargo_target = Path(__file__).parent.parent.parent / "target" / "release" / binary_name

    if cargo_target.exists():
        try:
            subprocess.run(
                [str(cargo_target), "--build", str(root)],
                capture_output=True,
                text=True,
                timeout=30,
                check=True,
            )
        except (subprocess.CalledProcessError, FileNotFoundError):
            pass

    yield root
    shutil.rmtree(tmp, ignore_errors=True)


@pytest.fixture
def empty_project() -> Generator[Path, None, None]:
    """Create a temporary directory without an index."""
    tmp = tempfile.mkdtemp()
    yield Path(tmp)
    shutil.rmtree(tmp, ignore_errors=True)
