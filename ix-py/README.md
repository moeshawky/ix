# ix-py (`moeix` on PyPI, `import ix` in Python)

- **Last Verified:** 2026-11-22 (v0.14.2)
- **Source:** `ix-py/python/ix/__init__.py`, `ix-py/python/ix/_ix.pyi`, `ix-py/src/{lib,index,types,safety,error}.rs`

Python bindings for [`ix`](https://github.com/moeshawky/ix) — sub-millisecond code search via sparse trigram indexing.

- **PyPI package name:** `moeix` (`ix-py/pyproject.toml:6`)
- **Cargo workspace member:** `ix-py` (`ix-py/Cargo.toml:2`, depending on Cargo crate `ix` at `ix-py/Cargo.toml:15`)
- **Python import & native module:** `import ix` (`module-name = "ix._ix"`, PEP 561 typed via `ix-py/python/ix/_ix.pyi` and `ix-py/python/ix/py.typed`)
- **Python compatibility:** CPython `>=3.10` (`ix-py/pyproject.toml:11`); Linux x86_64/aarch64 and Windows wheels built with `maturin` (`features = ["pyo3/extension-module", "full"]`)

## Install

```bash
pip install moeix
```

From source inside a virtualenv:

```bash
uv venv --python 3.12 ix-py/.venv
VIRTUAL_ENV="$PWD/ix-py/.venv" PATH="$PWD/ix-py/.venv/bin:$PATH" \
  maturin develop --manifest-path ix-py/Cargo.toml --release
```

## Quick Start

```python
import ix

# Build an index for a repository (requires default `notify` feature; releases the Python GIL)
build_stats = ix.build("/path/to/repo")
print(build_stats["files_indexed"], build_stats["duration_ms"])

# One-shot literal or regex search (releases the Python GIL during query execution)
result = ix.search("struct Config", "/path/to/repo", type_filter=["rs"])
for m in result.matches:
    print(f"{m.file_path}:{m.line_number}:{m.col}: {m.line_content}")

# Reuse an open memory-mapped Index (automatically reloads stale shards on rebuild)
with ix.Index("/path/to/repo", cache_mb=64) as idx:
    res = idx.search(r"fn\s+\w+", regex=True, max_results=20, context_lines=1)
    header = idx.stats()
    daemon = idx.service_status()
```

## Python API Summary (`v0.14.2`)

- `ix.build(path, *, max_file_size_mb=100, exclude_dirs=None) -> BuildStats` (`ix-py/python/ix/__init__.py:179`) — builds `.ix/shard.ix` and returns an 11-key `BuildStats` dictionary.
- `ix.search(pattern, path, **kwargs) -> SearchResult` (`ix-py/python/ix/__init__.py:158`) — supports `regex`, `context_lines`, `max_results`, `type_filter`, `multiline`, `case_insensitive`, `word_boundary`, `count_only`, and `files_only`.
- `ix.stats(path) -> IndexStats` (`ix-py/python/ix/__init__.py:217`) — returns all 11 shard header fields.
- `ix.service_status(path) -> ServiceStatus | None` (`ix-py/python/ix/__init__.py:252`) — inspects `.ix/beacon.json` and returns live `ixd` daemon status (11 fields) or `None`.
- `ix.Index(path, *, cache_mb=64)` (`ix-py/src/index.rs:280`) — context-managed mmapped shard handle with `search()`, `rebuild()`, `build()`, `stats()`, `service_status()`, `close()`, and properties `file_count`, `trigram_count`, `created_at`, `path`, `is_stale`. Calls after `close()` raise `RuntimeError("Index is closed")`.
- `ix.Pipeline(objective)` (`ix-py/src/safety.rs:29`) — wraps the 16-slot `llmosafe` C-ABI safety arena (objective `1..=1024` bytes; `process(text)` caps text at `10 MiB` and resets metrics on `""`; `get_decision()` returns the current `i32` decision code).

### Exceptions (`ix-py/src/error.rs:6-85`)

All `ix` errors inherit from `ix.IxError`: `IxIndexError`, `IxCorruptionError`, `IxIoError`, `IxRegexError`, `IxConfigError`, `IxWatcherError` (`notify` feature), `IxArchiveError` (`archive` feature), and `LLMOSafeError`.

## Feature Flags (`ix-py/Cargo.toml:23-28`)

| Flag | Default | Maps To | Effect when disabled |
|------|---------|---------|----------------------|
| `notify` | yes | `ix/notify` | `build()`, `rebuild()`, and `service_status()` raise `NotImplementedError` |
| `archive` | no | `ix/archive` | Archive search disabled (`zip`, `tar`, `flate2`) |
| `decompress` | no | `ix/decompress` | Compressed file search disabled (`flate2`, `bzip2`, `xz2`) |
| `full` | in wheel builds | All 3 above | Matches PyPI release wheels (`ix-py/pyproject.toml:45`) |

## Verification (`ix-py/.venv`, Verified 2026-11-22)

```bash
ix-py/.venv/bin/ruff check --config ix-py/pyproject.toml ix-py/
# Expected: All checks passed!

ix-py/.venv/bin/mypy --config-file ix-py/pyproject.toml ix-py/python/ix
# Expected: Success: no issues found in 2 source files

VIRTUAL_ENV="$PWD/ix-py/.venv" PATH="$PWD/ix-py/.venv/bin:$PATH" \
  maturin develop --manifest-path ix-py/Cargo.toml --release
ix-py/.venv/bin/pytest ix-py/tests/
# Expected: 34 passed
```
