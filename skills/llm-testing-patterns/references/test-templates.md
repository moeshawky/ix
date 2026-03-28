# Test Templates - Language-Agnostic Patterns

This document provides language-agnostic test templates that can be adapted to any testing framework.

## Template Categories

1. [Integration Test Templates](#integration-test-templates)
2. [Property-Based Test Templates](#property-based-test-templates)
3. [File I/O Test Templates](#file-io-test-templates)
4. [State Management Test Templates](#state-management-test-templates)
5. [Resume/Recovery Test Templates](#resumerecovery-test-templates)

---

## Integration Test Templates

### Template: Component Integration Test

```
Test: {component_a}_integrates_with_{component_b}

Setup:
  - Initialize component_a with test config
  - Initialize component_b with test config
  - Mock external dependencies

Execute:
  1. Pass test input through component_a
  2. Verify component_a output format
  3. Pass component_a output to component_b
  4. Verify component_b processes correctly

Assert:
  - component_a output is valid for component_b input
  - Data flows correctly between components
  - No data loss at boundaries
  - Error handling propagates correctly
```

### Template: Multi-Component Pipeline Test

```
Test: pipeline_processes_complete_workflow

Setup:
  - Initialize all pipeline components
  - Create test fixture representing realistic input
  - Mock external services (APIs, databases)

Execute:
  1. Feed test input into pipeline start
  2. Let pipeline process through all stages
  3. Capture output at pipeline end

Assert:
  - Pipeline completes without error
  - Output matches expected format
  - All stages were invoked in correct order
  - Intermediate state is correct
```

---

## Property-Based Test Templates

### Template: Invariant Property Test

```
Test: {function}_maintains_{invariant}

Property: For all valid inputs, {invariant} must hold

Generator: Generate valid inputs using property-based testing library

Assert:
  - {invariant}(result) == True for all generated inputs

Example invariants:
  - roundtrip: encode(decode(x)) == x
  - monotonic: if a <= b then f(a) <= f(b)
  - commutative: f(a, b) == f(b, a)
  - idempotent: f(f(x)) == f(x)
  - no_duplicates: len(unique(result)) == len(result)
```

### Template: Edge Case Discovery Test

```
Test: {function}_handles_all_edge_cases

Property: Function should not crash or produce invalid output for any input

Generator: Generate inputs including edge cases:
  - Empty collections
  - Single element
  - Maximum size
  - Null/None values
  - Boundary values (MIN, MAX)
  - Unicode strings
  - Deeply nested structures

Assert:
  - Function returns valid result OR raises expected exception
  - No unhandled exceptions
  - No infinite loops or hangs
```

### Template: State Machine Test

```
Test: {stateful_component}_state_transitions_valid

States: [Initial, Running, Paused, Completed, Error]

Transitions:
  - Initial -> Running (valid)
  - Running -> Paused (valid)
  - Running -> Completed (valid)
  - Running -> Error (valid)
  - Paused -> Running (valid)
  - Error -> Initial (valid for recovery)
  - Any other transition (invalid)

Generator: Generate random sequences of actions

Assert:
  - Each state transition is valid
  - Component remains in valid state after each action
  - Invalid transitions are rejected or handled gracefully
```

---

## File I/O Test Templates

### Template: File Processing Test

```
Test: {function}_processes_files_correctly

Setup:
  - Create temporary directory
  - Create test files with known content
  - Initialize file processor with temp directory

Execute:
  1. Call file processor with test file
  2. Capture output file(s)
  3. Verify file system state

Assert:
  - Input file was read correctly
  - Output file was created
  - Output file content is correct
  - No unintended file modifications
  - Temp files cleaned up (if applicable)

Cleanup:
  - Delete temporary directory
```

### Template: Double-Processing Prevention Test

```
Test: {processor}_does_not_process_own_output

Setup:
  - Create temp directory
  - Create source file: "document.txt"
  - Process once to create: "document_DISTILLED.txt"

Execute:
  1. Run processor on directory containing both files
  2. Check if "document_DISTILLED_DISTILLED.txt" was created

Assert:
  - "document_DISTILLED_DISTILLED.txt" does NOT exist
  - Processor skips files with "_DISTILLED" suffix
  - Only original files are processed
```

### Template: File Path Handling Test

```
Test: {processor}_handles_various_file_paths

Test cases:
  - Relative paths: "subdir/file.txt"
  - Absolute paths: "/full/path/to/file.txt"
  - Paths with spaces: "my documents/file.txt"
  - Unicode paths: "文档/file.txt"
  - Deep nesting: "a/b/c/d/e/f/file.txt"
  - Symbolic links (if supported)

Assert:
  - All valid paths processed correctly
  - Invalid paths handled gracefully
  - Path traversal attacks blocked
```

---

## State Management Test Templates

### Template: State Persistence Test

```
Test: {component}_persists_state_correctly

Setup:
  - Initialize component
  - Perform series of state-changing operations
  - Capture state checkpoint

Execute:
  1. Save state to disk
  2. Create new component instance
  3. Load state from disk
  4. Verify state matches checkpoint

Assert:
  - Saved state is valid
  - Loaded state matches saved state
  - Component behavior is identical after reload
```

### Template: State Version Compatibility Test

```
Test: {state_loader}_handles_version_compatibility

Test cases:
  - Load state from version N-1
  - Load state from version N-2
  - Load empty state (no file)
  - Load corrupted state (invalid JSON)

Assert:
  - Older versions load with warnings or migration
  - Missing state initializes to defaults
  - Corrupted state fails gracefully with clear error
```

### Template: Concurrent State Access Test

```
Test: {state_manager}_handles_concurrent_access

Setup:
  - Create shared state
  - Spawn multiple concurrent operations

Execute:
  - Multiple threads/processes read/write state
  - Verify no race conditions

Assert:
  - No data corruption
  - No deadlock
  - Final state is consistent
  - Operations are properly serialized (if needed)
```

---

## Resume/Recovery Test Templates

### Template: Resume After Interruption Test

```
Test: {pipeline}_resumes_correctly_after_interruption

Setup:
  - Create checkpoint/state file
  - Process partial data
  - Simulate interruption (kill process, throw exception)

Execute:
  1. Restart pipeline with resume flag
  2. Verify checkpoint is loaded
  3. Verify processing continues from checkpoint
  4. Verify no duplicate processing

Assert:
  - Checkpoint state is valid
  - Processing resumes from correct point
  - No work is duplicated
  - Final result is complete and correct
```

### Template: Recovery from Corrupted State Test

```
Test: {system}_recovers_from_corrupted_state

Test cases:
  - Corrupted checkpoint file
  - Missing checkpoint file
  - Partial checkpoint (incomplete write)
  - Version mismatch in checkpoint

Assert:
  - System detects corruption
  - System provides clear error message
  - System can recover (reprocess from start or last known good)
  - No data loss after recovery
```

### Template: Idempotent Processing Test

```
Test: {processor}_is_idempotent

Execute:
  1. Process input
  2. Process same input again
  3. Compare results

Assert:
  - Results are identical
  - No duplicate side effects
  - No duplicate output files
  - Processing twice == processing once
```

---

## Test Fixture Templates

### Template: Test Data Fixtures

```
fixtures/
├── minimal/          # Minimal valid input
├── typical/          # Typical/expected input
├── edge_cases/       # Edge case inputs
│   ├── empty.*
│   ├── single_element.*
│   ├── maximum_size.*
│   └── unicode.*
├── adversarial/      # Malicious/problematic input
│   ├── sql_injection.*
│   ├── path_traversal.*
│   └── overflow.*
└── corrupted/        # Invalid/corrupted data
    ├── truncated.*
    ├── invalid_format.*
    └── malformed.*
```

### Template: Environment Fixtures

```
environments/
├── minimal.env       # Only required env vars
├── typical.env       # Normal configuration
├── production.env    # Production-like config
└── invalid.env       # Missing/invalid config
```

---

## Assertion Templates

### Template: Semantic Assertions

```
# Instead of just checking output
assert result == expected

# Check semantic properties
assert result.is_valid()
assert result.satisfies_specification(spec)
assert result.behavior_matches(expected_behavior)

# Check invariants
assert invariant_holds(result)
assert no_violations_of(rules, result)
```

### Template: Integration Assertions

```
# Check component integration
assert component_a.output_valid_for(component_b)
assert data_flow_complete(pipeline_trace)
assert all_components_invoked(trace)

# Check file system effects
assert expected_files_created(output_dir)
assert no_unintended_modifications(input_dir)
assert file_contents_match(output_file, expected)
```

### Template: Performance Assertions

```
# Check performance bounds
assert execution_time < budget_ms
assert memory_usage < budget_mb
assert complexity_is_linear_or_better()

# Check no regressions
assert current_performance >= baseline_performance
```
