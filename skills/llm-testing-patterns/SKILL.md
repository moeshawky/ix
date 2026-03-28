---
name: llm-testing-patterns
description: Comprehensive testing patterns for LLM-generated code and AI agents. Covers 9 failure mode categories (hallucinated APIs, security vulnerabilities, edge cases, state management bugs, model version drift), property-based testing for invariants, integration testing for file I/O, cascade detection for multi-failure diagnosis, and test oracle patterns. Use when writing tests for AI-generated code, testing LLM agents, validating code correctness beyond happy paths, or designing test suites that catch LLM-specific bugs. Keywords: LLM testing, AI agent testing, property-based testing, integration testing, failure modes, test patterns, hypothesis testing, guardrails.
---

# LLM Testing Patterns

> **Strategy design**: Use `productive_reason` to plan your test strategy before writing tests. Use `scratchpad` to track which failure modes have been covered and which remain open.

Testing LLM-generated code requires fundamentally different approaches than testing human-written code. LLMs fail in systematic, predictable patterns that traditional testing misses.

## Why LLM Testing Is Different

| Human Code Bugs | LLM Code Bugs |
|-----------------|---------------|
| Random, scattered | Systematic, patterned |
| Single point failures | Cascade failures |
| Syntax errors common | Plausible but wrong logic |
| Edge cases occasionally missed | Edge cases systematically missed |

**Key insight**: LLM code passes unit tests but fails integration tests. It works for happy paths but breaks on edge cases that humans rarely forget.

## Usage Pattern

### AOP v3 Screening Order (Fail Fast)

When testing LLM-generated code, screen in this order and **stop at first failure**:

1. **G-HALL** — Run import validation. If hallucinated APIs exist, nothing else matters.
2. **G-SEC** — Security vulnerabilities compound everything else.
3. **G-EDGE** — Edge cases are where LLMs fail most predictably.
4. **G-SEM** — Semantic correctness requires the code to even run correctly.
5. **G-ERR** — Error handling gaps mask deeper integration failures.
6. **G-CTX** — Context/environment dependencies surface late but break everything.
7. **G-DRIFT** — Regression against golden files catches version drift.

Skip G-PERF and G-DEP for initial validation — they're optimization, not correctness.

## The 9 Failure Mode Categories

LLM-generated code fails in predictable patterns. Learn these patterns to design tests that catch them.

### G-HALL: Hallucinated APIs
Code references packages, methods, or properties that don't exist.

**Test pattern**: Import validation, API surface verification
```python
# Test: Verify imports exist
def test_no_hallucinated_imports():
    imports = extract_imports(generated_code)
    for pkg in imports:
        assert package_exists(pkg), f"Hallucinated package: {pkg}"
```

### G-SEC: Security Vulnerabilities
Code works but fails securely. SQL injection, auth bypass, data leaks.

**Test pattern**: Adversarial input testing, security scanners
```python
# Test: SQL injection resilience
def test_sql_injection():
    malicious_inputs = ["'; DROP TABLE users; --", "1 OR 1=1"]
    for inp in malicious_inputs:
        result = process_input(inp)
        assert not result.executes_sql()
```

### G-PERF: Performance Anti-Patterns
O(n²) where O(n) exists, inefficient data structures, unnecessary allocations.

**Test pattern**: Performance budgets, complexity verification
```python
# Test: Complexity bounds
def test_algorithm_complexity():
    sizes = [100, 1000, 10000]
    times = [measure_time(process, n) for n in sizes]
    assert is_linear_or_better(times), "O(n²) detected"
```

### G-ERR: Missing Error Handling
Happy path only. No null checks, no error boundaries, stack traces leak.

**Test pattern**: Fault injection, error path coverage
```python
# Test: Error path handling
def test_handles_errors():
    bad_inputs = [None, "", -1, MAX_INT, "x" * 10000]
    for inp in bad_inputs:
        result = process(inp)  # Should not crash
        assert result is not None or raises_expected_error()
```

### G-EDGE: Missing Edge Cases
Empty arrays, null values, boundary conditions, unicode.

**Test pattern**: Edge case matrix, property-based testing
```python
# Test: Edge case matrix
@pytest.mark.parametrize("input", [
    [],           # empty
    [None],       # null element
    [1, 2, 3],    # normal
    [-MAX, MAX],  # boundaries
])
def test_edge_cases(input):
    result = process(input)
    assert result is valid_or_handled()
```

### G-DEP: Outdated Dependencies
Deprecated APIs, old library versions, reintroduced vulnerabilities.

**Test pattern**: Dependency scanning, version checks
```python
# Test: Dependency freshness
def test_dependencies_current():
    deps = parse_requirements()
    for dep in deps:
        latest = fetch_latest_version(dep.name)
        assert dep.version >= latest.min_safe
```

### G-CTX: Missing Context Dependencies
Works in isolation but fails when integrated. Missing env vars, configs.

**Test pattern**: Integration testing, environment simulation
```python
# Test: Environment completeness
def test_all_deps_documented():
    env_refs = extract_env_vars(generated_code)
    documented = parse_env_docs()
    for ref in env_refs:
        assert ref in documented, f"Undocumented env var: {ref}"
```

### G-SEM: Semantic Errors
Plausible but wrong logic. Code looks correct but implements wrong behavior.

**Test pattern**: Behavioral testing, oracle comparison
```python
# Test: Behavior matches specification
def test_semantic_correctness():
    for test_case in specification.examples:
        result = generated_function(test_case.input)
        assert result == test_case.expected_output
```

### G-DRIFT: Model Version Drift
Same prompt, different model version = different output. Tests pass today, fail tomorrow.

**Test pattern**: Golden file comparison, semantic equivalence
```python
# Test: No regression from model updates
def test_no_regression():
    result = process(test_input)
    golden = load_golden("test_input_expected.txt")
    assert semantic_equivalence(result, golden), \
        f"Output drifted from golden file. Review and update if intentional."
```

```rust
// Rust: golden file regression test
#[test]
fn test_no_drift() {
    let result = process(TEST_INPUT);
    let golden = include_str!("golden/expected_output.txt");
    assert_eq!(result.trim(), golden.trim(),
        "Output drifted from golden file");
}
```

## Cascade Detection

> **AOP v3 Rule**: When 2+ failure modes appear in the same output, the problem is upstream. Don't fix individual bugs — fix the flow.

| Cascade Pattern | Root Cause | Action |
|-----------------|------------|--------|
| G-HALL + G-SEC + G-SEM | Prompt is fundamentally wrong | Rewrite prompt with concrete examples |
| G-EDGE + G-ERR | Missing domain knowledge | Provide edge case examples in context |
| G-PERF + G-DEP | Outdated training data | Pin library versions in prompt, provide current API docs |
| G-HALL + G-DRIFT | Model downgrade or version change | Check model version, test against known-good model |
| G-SEM + G-CTX | Integration assumptions wrong | Add integration test fixtures, mock environment |

**Three consecutive failures = STOP.** Question the architecture, not the individual bugs. See `advanced-debugging` skill for the full escalation protocol.

## Model-Class Test Strategy

Different model classes produce different failure signatures. Tailor your test emphasis to the model being tested.

| Model Class | Typical Failure Profile | Test Emphasis |
|-------------|------------------------|---------------|
| **Code-Focused** (e.g. DeepSeek Coder, CodeLlama) | Template Fitting — copies structural patterns without semantic understanding | Add Template Fitting detection tests: verify output is NOT a near-copy of training examples; check that variable names/structures vary meaningfully with problem constraints |
| **Large Reasoning** (e.g. GPT-4o, Claude Opus) | G-HALL on niche APIs; G-SEM on multi-step logic | Prioritize G-HALL + G-SEM; use behavioral oracles against reference implementations |
| **Text-Only Reasoning** (e.g. o1, o3-mini) | G-CTX — misses environmental constraints; G-DRIFT on format changes | Heavy G-CTX coverage; golden file tests with strict format assertions |
| **Reliable Instruction** (e.g. GPT-3.5-turbo fine-tunes) | G-EDGE + G-ERR — follows instructions but drops error paths and boundary handling | Saturate G-EDGE matrix; fault injection on every public interface |

### Template Fitting Detection (Code-Focused Models)

Code-focused models frequently fit to training templates rather than solving the actual problem. Detect this pattern:

```python
# Test: Output varies meaningfully with problem constraints
def test_not_template_fitted():
    result_a = generate(spec_a)
    result_b = generate(spec_b)  # spec_b has different constraints
    similarity = structural_similarity(result_a, result_b)
    assert similarity < 0.85, \
        "Outputs too similar — possible template fitting, not problem solving"
```

```python
# Test: Variable names reflect problem domain, not a generic template
def test_domain_appropriate_naming():
    result = generate(domain_spec)
    generic_names = {"data", "item", "obj", "thing", "value", "temp", "x", "y"}
    used_names = extract_identifiers(result)
    assert len(used_names & generic_names) / len(used_names) < 0.3, \
        "High ratio of generic names — suspect template fitting"
```

### Hallucination Checks (Reasoning Models)

Reasoning models are more likely to fabricate plausible-sounding API details when uncertain:

```python
# Test: All referenced APIs are verifiable in the target library version
def test_no_fabricated_apis():
    apis = extract_api_calls(generated_code)
    for api in apis:
        assert api_exists_in_version(api.name, api.module, TARGET_VERSION), \
            f"Fabricated or version-mismatched API: {api}"
```

## Test Suite Structure

```
tests/
├── smoke/           # G-HALL, G-SEC — does it even run safely?
│   ├── test_imports_exist.py
│   └── test_no_injection.py
├── unit/            # G-EDGE, G-ERR — edge cases and error handling
│   ├── test_edge_cases.py
│   └── test_error_handling.py
├── integration/     # G-CTX — does it work in context?
│   ├── test_file_io.py
│   └── test_state_management.py
├── property/        # G-SEM — behavioral correctness via invariants
│   └── test_invariants.py
└── regression/      # G-DRIFT — golden files for drift detection
    ├── test_golden.py
    └── golden/
        ├── expected_output_1.txt
        └── expected_output_2.txt
```

**Run order**: smoke → unit → integration → property → regression. Fail fast — if smoke fails, skip the rest.

## Testing Layers

### Layer 1: Unit Testing
Test individual components in isolation.

**Focus**: Input/output correctness, edge cases, error handling
**Misses**: Integration bugs, state management bugs, file I/O bugs

### Layer 2: Integration Testing
Test components working together.

**Focus**: File I/O, state management, API boundaries, data flow
**Catches**: The bugs that unit tests miss

**Key integration test patterns**:
- File system interaction tests
- State persistence/resume tests
- Multi-component coordination tests

### Layer 3: Property-Based Testing
Define invariants that must hold for ALL inputs.

**Focus**: Edge cases you didn't think of, fuzzing, state machine testing
**Catches**: Systematic LLM failures on boundaries

```python
# Property: No double-processing
@given(file_path=st.text())
def test_no_double_processing(file_path):
    result1 = process(file_path)
    result2 = process(result1.output_path)
    assert "_DISTILLED" not in result2.output_path
```

### Layer 4: Behavioral Testing
Test capabilities, not just outputs.

**Categories**:
- **Capability tests**: Can it handle X?
- **Robustness tests**: Does it handle variations?
- **Edge case tests**: What about null, empty, boundary?
- **Invariance tests**: Same result for equivalent inputs?

## Test Oracle Patterns

How do you know the test passed for the right reason?

### 1. Reference Oracle
Compare against known-good implementation.
```python
def test_matches_reference():
    result = generated_function(input)
    expected = reference_implementation(input)
    assert result == expected
```

### 2. Metamorphic Oracle
Related inputs produce related outputs.
```python
def test_metamorphic():
    result1 = process(input)
    result2 = process(input + "_variant")
    assert is_consistent(result1, result2)
```

### 3. Contract Oracle
Verify preconditions/postconditions.
```python
def test_contract():
    assert precondition(input)
    result = process(input)
    assert postcondition(result)
```

### 4. Invariant Oracle
Properties that must always hold.
```python
def test_invariant():
    result = process(input)
    assert invariant_holds(result)
```

## Quick Reference: Copy-Paste Test Patterns

### Test for Hallucinated Imports (Python)
```python
def test_imports_exist():
    for pkg in extract_imports(generated_code):
        __import__(pkg)  # Raises ImportError if hallucinated
```

### Test for Hallucinated APIs (Rust)
```rust
#[test]
fn test_no_hallucinated_methods() {
    // Verify the generated code only calls methods that exist on the type
    let val = MyStruct::new();
    // If this compiles, the methods exist. Hallucinated methods = compile error.
    let _ = val.known_method();
}
```

### Test for SQL Injection
```python
@pytest.mark.parametrize("inp", ["'; DROP TABLE users; --", "1 OR 1=1", "\" OR \"\"=\""])
def test_sql_injection(inp):
    result = process(inp)
    assert not contains_raw_sql(result)
```

### Test for Edge Cases
```python
@pytest.mark.parametrize("inp", [None, "", [], {}, 0, -1, MAX_INT, float('nan'), float('inf')])
def test_edge_cases(inp):
    result = process(inp)
    assert result is not None or raises_expected()
```

### Test for Edge Cases (Rust)
```rust
#[test]
fn test_edge_cases() {
    assert!(process("").is_ok());           // empty
    assert!(process(" ").is_ok());          // whitespace
    assert!(process("a").is_ok());          // single char
    assert!(process(&"x".repeat(100_000)).is_ok());  // very long
    assert!(process("مرحبا").is_ok());     // unicode/Arabic
    assert!(process("\0\0\0").is_ok());     // null bytes
}
```

### Golden File Regression (Rust)
```rust
#[test]
fn test_golden_output() {
    let result = process(KNOWN_INPUT);
    let golden = include_str!("golden/expected.txt");
    assert_eq!(normalize(result), normalize(golden),
        "Output drifted. If intentional, update golden file.");
}
```

### Property-Based Invariant (Rust)
```rust
use proptest::prelude::*;

proptest! {
    #[test]
    fn encode_decode_roundtrip(input in ".*") {
        let encoded = encode(&input);
        let decoded = decode(&encoded).unwrap();
        prop_assert_eq!(input, decoded);
    }
}
```

## Property-Based Testing Libraries

| Language | Library | Import | Example |
|----------|---------|--------|---------|
| Python | Hypothesis | `from hypothesis import given, strategies as st` | `@given(st.integers())` |
| Rust | Proptest | `use proptest::prelude::*;` | `proptest! { #[test] fn ... }` |
| Rust | Quickcheck | `use quickcheck_macros::quickcheck;` | `#[quickcheck] fn ...` |
| JavaScript | fast-check | `import fc from 'fast-check';` | `fc.assert(fc.property(fc.integer(), ...))` |
| Go | gopter | `import "github.com/leanovate/gopter"` | `properties.NewProperty()` |
| Java | jqwik | `@Property` | `@ForAll int x` |

## Quick Reference: Test Patterns by Failure Mode

| Failure Mode | Test Pattern | Key Assertion | Priority |
|--------------|--------------|---------------|----------|
| G-HALL | Import validation | `assert package_exists(pkg)` | 1 (first) |
| G-SEC | Adversarial testing | `assert not vulnerable(result)` | 2 |
| G-EDGE | Edge case matrix | `assert handles_empty_null_boundary()` | 3 |
| G-SEM | Behavioral testing | `assert matches_specification()` | 4 |
| G-ERR | Fault injection | `assert handles_gracefully(error)` | 5 |
| G-CTX | Environment simulation | `assert all_deps_documented()` | 6 |
| G-DRIFT | Golden file comparison | `assert matches_golden(result)` | 7 |
| G-PERF | Complexity bounds | `assert is_linear_or_better(times)` | 8 (last) |
| G-DEP | Dependency scanning | `assert version >= min_safe` | 9 (last) |

## When to Use This Skill

Use when:
- Writing tests for AI-generated code
- Testing LLM agents or pipelines
- Designing test suites for file I/O and state management
- Validating code correctness beyond happy paths
- Implementing property-based testing
- Investigating test failures that seem random (check G-DRIFT)
- Seeing multiple failure modes at once (check Cascade Detection)

## References

- **failure-modes.md**: Detailed patterns for each of the 9 failure modes
- **test-templates.md**: Language-agnostic test templates
- **integration-patterns.md**: File I/O, state management, resume testing patterns
