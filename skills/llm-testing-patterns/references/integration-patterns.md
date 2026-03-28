# Integration Patterns - File I/O, State Management, Resume Testing

This document focuses on integration testing patterns that catch the bugs unit tests miss.

## The Integration Testing Gap

Unit tests test components in isolation. Integration tests test components working together.

**Bugs found in distill project**:
1. `_DISTILLED_DISTILLED` files created - inventory didn't filter output files
2. Output tracking used wrong path format - flat filename vs relative path
3. Resume logic didn't properly restore state

**Root cause**: No integration tests for file I/O, state management, or resume scenarios.

---

## File I/O Testing Patterns

### Pattern: Input/Output Boundary Testing

Test that file inputs are correctly transformed to file outputs.

```python
# Template: File Processing Integration Test
def test_file_processing_pipeline(tmp_path):
    """
    Integration test: Verify file processing works end-to-end.
    
    Tests:
    - File discovery
    - File reading
    - Processing
    - File writing
    - Output tracking
    """
    # Setup: Create realistic input
    input_file = tmp_path / "source.md"
    input_file.write_text("# Test Document\n\nContent here.")
    
    # Execute: Run full pipeline
    result = process_directory(tmp_path)
    
    # Assert: Verify complete pipeline
    assert result.success
    assert (tmp_path / "source_DISTILLED.md").exists()
    assert "source_DISTILLED.md" in result.outputs_created
```

### Pattern: Double-Processing Prevention

Test that output files are never processed as input.

```python
# Template: Double-Processing Prevention Test
def test_no_double_processing(tmp_path):
    """
    Integration test: Verify output files are filtered from input.
    
    This catches the _DISTILLED_DISTILLED bug.
    """
    # Setup: Create source and a fake output file
    (tmp_path / "doc1.md").write_text("content")
    (tmp_path / "doc1_DISTILLED.md").write_text("already processed")
    
    # Execute: Run inventory
    files = discover_files(tmp_path)
    
    # Assert: Output files filtered
    assert "doc1.md" in files
    assert "doc1_DISTILLED.md" not in files
    
    # Execute: Process directory
    result = process_directory(tmp_path)
    
    # Assert: No double-processing
    assert not (tmp_path / "doc1_DISTILLED_DISTILLED.md").exists()
```

### Pattern: Path Format Consistency

Test that paths are handled consistently across components.

```python
# Template: Path Format Consistency Test
def test_path_format_consistency(tmp_path):
    """
    Integration test: Verify path formats are consistent.
    
    This catches the flat-filename-vs-relative-path bug.
    """
    # Setup: Create nested structure
    subdir = tmp_path / "subdir"
    subdir.mkdir()
    (subdir / "doc.md").write_text("content")
    
    # Execute: Process and track outputs
    result = process_directory(tmp_path)
    
    # Assert: Paths are relative to source_dir
    for output in result.outputs_created:
        assert output.startswith("subdir/") or "/" not in output
        assert "\\" not in output  # No Windows paths in tracking
        assert Path(tmp_path / output).exists()
```

### Pattern: File System State Verification

Test that file system state matches expectations after processing.

```python
# Template: File System State Test
def test_filesystem_state_after_processing(tmp_path):
    """
    Integration test: Verify file system state is correct.
    """
    # Setup
    create_test_files(tmp_path, ["a.md", "b.md", "c.md"])
    
    # Execute
    process_directory(tmp_path)
    
    # Assert: Check actual file system
    output_files = list(tmp_path.glob("**/*_DISTILLED.md"))
    assert len(output_files) == 3
    
    # Assert: Check tracking matches reality
    tracked = load_outputs_created(tmp_path / ".scratchpad.json")
    actual = [str(f.relative_to(tmp_path)) for f in output_files]
    assert set(tracked) == set(actual)
```

---

## State Management Testing Patterns

### Pattern: State Persistence/Restoration

Test that state is correctly saved and restored.

```python
# Template: State Persistence Test
def test_state_persistence(tmp_path):
    """
    Integration test: Verify state saves and restores correctly.
    """
    # Setup: Create initial state
    state = Scratchpad(version="1.2")
    state.outputs_created = ["file1_DISTILLED.md", "file2_DISTILLED.md"]
    state.save(tmp_path / ".scratchpad.json")
    
    # Execute: Load state
    loaded = Scratchpad.load(tmp_path / ".scratchpad.json")
    
    # Assert: State preserved
    assert loaded.version == "1.2"
    assert loaded.outputs_created == ["file1_DISTILLED.md", "file2_DISTILLED.md"]
```

### Pattern: State Version Compatibility

Test backward compatibility with older state versions.

```python
# Template: State Version Compatibility Test
def test_backward_compatibility(tmp_path):
    """
    Integration test: Old state versions load correctly.
    """
    # Setup: Create v1.1 state (without outputs_created)
    v1_1_state = {
        "version": "1.1",
        "processed_files": ["a.md", "b.md"],
        # Note: no "outputs_created" field
    }
    (tmp_path / ".scratchpad.json").write_text(json.dumps(v1_1_state))
    
    # Execute: Load with current version
    loaded = Scratchpad.load(tmp_path / ".scratchpad.json")
    
    # Assert: Backward compatible
    assert loaded.version == "1.1"
    assert loaded.outputs_created == []  # Default for missing field
    assert loaded.processed_files == ["a.md", "b.md"]
```

### Pattern: Concurrent State Access

Test that state is safe for concurrent access.

```python
# Template: Concurrent State Test
def test_concurrent_state_access(tmp_path):
    """
    Integration test: Multiple processes can safely access state.
    """
    from concurrent.futures import ThreadPoolExecutor
    
    state_file = tmp_path / ".scratchpad.json"
    
    def modify_state(i):
        state = Scratchpad.load(state_file)
        state.outputs_created.append(f"file_{i}_DISTILLED.md")
        time.sleep(0.01)  # Encourage race condition
        state.save(state_file)
        return i
    
    # Execute: Concurrent modifications
    with ThreadPoolExecutor(max_workers=5) as executor:
        results = list(executor.map(modify_state, range(5)))
    
    # Assert: No lost updates (file locking or atomic saves)
    final_state = Scratchpad.load(state_file)
    # All modifications should be present or merged
    assert len(final_state.outputs_created) >= 1
```

---

## Resume/Recovery Testing Patterns

### Pattern: Resume After Interruption

Test that processing can resume from checkpoint.

```python
# Template: Resume Test
def test_resume_after_interruption(tmp_path):
    """
    Integration test: Processing resumes correctly after interruption.
    """
    # Setup: Create files
    files = ["a.md", "b.md", "c.md"]
    create_test_files(tmp_path, files)
    
    # Execute: Partial processing
    process_partial(tmp_path, stop_after=1)  # Process only 'a.md'
    
    # Assert: Checkpoint created
    state = Scratchpad.load(tmp_path / ".scratchpad.json")
    assert "a.md" in state.processed_files
    assert "b.md" not in state.processed_files
    
    # Execute: Resume
    result = resume_processing(tmp_path)
    
    # Assert: Remaining files processed
    assert "b.md" in result.processed_files
    assert "c.md" in result.processed_files
    
    # Assert: No re-processing
    assert result.files_processed == 2  # Only b and c, not a again
```

### Pattern: Idempotent Resume

Test that resume produces same result as full processing.

```python
# Template: Idempotent Resume Test
def test_resume_is_idempotent(tmp_path):
    """
    Integration test: Resume produces same result as fresh start.
    """
    # Setup
    create_test_files(tmp_path, ["a.md", "b.md"])
    
    # Execute: Full processing
    result1 = process_directory(tmp_path)
    output1 = (tmp_path / "a_DISTILLED.md").read_text()
    
    # Setup: Clear outputs, keep state
    for f in tmp_path.glob("*_DISTILLED.md"):
        f.unlink()
    
    # Execute: Resume (should not re-process)
    result2 = resume_processing(tmp_path)
    
    # Assert: No new outputs (idempotent)
    assert not (tmp_path / "a_DISTILLED.md").exists()
    assert result2.files_processed == 0
```

### Pattern: Recovery from Corrupted State

Test graceful handling of corrupted state.

```python
# Template: Corrupted State Recovery Test
def test_handles_corrupted_state(tmp_path):
    """
    Integration test: System recovers from corrupted state.
    """
    # Setup: Corrupted state file
    (tmp_path / ".scratchpad.json").write_text("{ invalid json }")
    
    # Execute: Should not crash
    result = process_directory(tmp_path)
    
    # Assert: Graceful handling
    assert result.success  # Or result.recovered == True
    assert result.warnings  # Warning about corrupted state
    
    # Assert: New valid state created
    state = Scratchpad.load(tmp_path / ".scratchpad.json")
    assert state.is_valid()
```

---

## Multi-Component Integration Patterns

### Pattern: Pipeline Integration Test

Test complete pipeline from input to output.

```python
# Template: Pipeline Integration Test
def test_full_pipeline_integration(tmp_path):
    """
    Integration test: All phases work together.
    
    Phases: inventory -> triage -> deep_read -> converge -> output
    """
    # Setup: Realistic input
    create_paper_directory(tmp_path, papers=3)
    
    # Execute: Full pipeline
    result = run_pipeline(
        source_dir=tmp_path,
        output_file=tmp_path / "output.md"
    )
    
    # Assert: All phases executed
    assert result.phases_completed == ["inventory", "triage", "deep_read", "converge", "output"]
    
    # Assert: Output is valid
    output = (tmp_path / "output.md").read_text()
    assert len(output) > 100  # Real content, not just index
    assert "# Consolidated" in output  # Proper formatting
    
    # Assert: Scratchpad updated
    state = Scratchpad.load(tmp_path / ".scratchpad.json")
    assert len(state.outputs_created) > 0
```

### Pattern: Error Propagation Test

Test that errors propagate correctly through pipeline.

```python
# Template: Error Propagation Test
def test_error_propagation(tmp_path):
    """
    Integration test: Errors propagate correctly through pipeline.
    """
    # Setup: Create file that will cause error
    (tmp_path / "corrupt.md").write_text("\x00\x00\x00")  # Binary data
    
    # Execute: Should handle error
    result = run_pipeline(source_dir=tmp_path)
    
    # Assert: Error caught and reported
    assert "corrupt.md" in result.errors
    assert result.partial_success  # Other files processed
    
    # Assert: Pipeline didn't crash
    assert result.completed
```

---

## Test Infrastructure Patterns

### Pattern: Test Fixtures for Integration Tests

```python
# Template: Integration Test Fixtures
import pytest
import tempfile
import shutil

@pytest.fixture
def temp_workspace():
    """Create isolated workspace for each test."""
    workspace = tempfile.mkdtemp()
    yield Path(workspace)
    shutil.rmtree(workspace)  # Cleanup

@pytest.fixture
def sample_papers(temp_workspace):
    """Create sample papers for testing."""
    papers_dir = temp_workspace / "papers"
    papers_dir.mkdir()
    for i in range(3):
        (papers_dir / f"paper_{i}.md").write_text(f"# Paper {i}\n\nContent.")
    return papers_dir

@pytest.fixture
def mock_llm_client():
    """Mock LLM client for deterministic testing."""
    with mock.patch('distill.engine.client.llm_call') as m:
        m.return_value = {"content": "Mocked response"}
        yield m
```

### Pattern: Test Helpers

```python
# Template: Integration Test Helpers
def assert_file_created(path: Path):
    """Assert file was created and is valid."""
    assert path.exists(), f"File not created: {path}"
    assert path.stat().st_size > 0, f"File is empty: {path}"

def assert_no_double_processing(tmp_path: Path):
    """Assert no _DISTILLED_DISTILLED files exist."""
    double_processed = list(tmp_path.glob("**/*_DISTILLED_DISTILLED.*"))
    assert len(double_processed) == 0, f"Double-processed files: {double_processed}"

def assert_tracking_matches_files(tmp_path: Path, state_file: Path):
    """Assert scratchpad tracking matches actual files."""
    state = Scratchpad.load(state_file)
    actual_outputs = [str(f.relative_to(tmp_path)) 
                      for f in tmp_path.glob("**/*_DISTILLED.md")]
    assert set(state.outputs_created) == set(actual_outputs)
```

---

## Checklist: Integration Test Coverage

Use this checklist to ensure integration tests cover critical paths:

- [ ] File discovery filters output files
- [ ] Path formats are consistent across components
- [ ] State saves and loads correctly
- [ ] Backward compatibility with old state versions
- [ ] Resume continues from checkpoint
- [ ] Resume doesn't re-process completed work
- [ ] Corrupted state is handled gracefully
- [ ] Pipeline completes end-to-end
- [ ] Errors propagate correctly
- [ ] File system state matches tracking
- [ ] No double-processing of output files
- [ ] Output contains actual content, not just index
