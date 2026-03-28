# LLM Failure Modes - Detailed Patterns

This document provides detailed patterns, examples, and test strategies for each of the 8 LLM failure mode categories.

## G-HALL: Hallucinated APIs

### What It Looks Like
- Importing packages that don't exist
- Calling methods that sound plausible but aren't in the API
- Using properties that "should be there"

### Why LLMs Do This
LLMs learn patterns, not facts. They generate plausible-looking code based on training data patterns.

### Detection Rate
1 in 5 AI code samples contains references to fake libraries (Augment Code research).

### Test Strategy
1. **Import verification**: Check all imports against package registries
2. **API surface validation**: Verify method signatures match documentation
3. **Runtime smoke test**: Actually import and call the code

### Test Patterns

```python
# Pattern 1: Import existence check
def test_imports_exist():
    """Verify all imports resolve to real packages."""
    imports = extract_imports(generated_code)
    for pkg, module in imports:
        try:
            __import__(pkg)
        except ImportError:
            raise AssertionError(f"Hallucinated: {pkg}.{module}")

# Pattern 2: Method signature validation
def test_method_signatures():
    """Verify method calls match actual API signatures."""
    calls = extract_method_calls(generated_code)
    for obj, method, args in calls:
        actual_sig = get_signature(obj, method)
        assert matches_signature(args, actual_sig)

# Pattern 3: Package registry check
def test_packages_in_registry():
    """Verify packages exist in official registries."""
    packages = extract_package_imports(generated_code)
    for pkg in packages:
        registry = detect_registry(pkg)  # npm, pypi, cargo, etc.
        assert package_in_registry(pkg, registry)
```

### Common Hallucinations
- `react-utils` (doesn't exist, use `react` utilities)
- `lodash/fp/map` (should be `lodash/map`)
- `python-request` (should be `requests`)

---

## G-SEC: Security Vulnerabilities

### What It Looks Like
- SQL queries built with string concatenation
- Authentication checks that can be bypassed
- Error messages that leak internal state
- Unsanitized user input in commands

### Why LLMs Do This
LLMs optimize for making code work, not making code secure. Security requires adversarial thinking.

### Detection Rate
45% of AI-generated code contains security vulnerabilities (Veracode 2025).

### Test Strategy
1. **Static analysis**: Run security scanners (CodeQL, Semgrep)
2. **Adversarial testing**: Test with malicious inputs
3. **Error message audit**: Check what errors reveal

### Test Patterns

```python
# Pattern 1: SQL injection test
@pytest.mark.parametrize("malicious", [
    "'; DROP TABLE users; --",
    "1 OR 1=1",
    "admin'--",
    "1; DELETE FROM users WHERE 1=1",
])
def test_sql_injection(malicious):
    result = query_database(malicious)
    assert not result.allows_injection()
    assert not result.leaks_error_details()

# Pattern 2: Authentication bypass test
def test_auth_bypass():
    """Test common auth bypass attempts."""
    bypasses = [
        {"user": "admin", "password": ""},
        {"user": "admin'--", "password": "anything"},
        {"user": "admin", "password": "' OR '1'='1"},
    ]
    for attempt in bypasses:
        assert not authenticate(attempt)

# Pattern 3: Error message sanitization
def test_error_sanitization():
    """Verify errors don't leak internals."""
    try:
        process(invalid_input)
    except Exception as e:
        error_msg = str(e)
        assert "password" not in error_msg.lower()
        assert "token" not in error_msg.lower()
        assert "secret" not in error_msg.lower()
        assert not contains_stack_trace(error_msg)

# Pattern 4: Command injection test
@pytest.mark.parametrize("cmd", [
    "; rm -rf /",
    "| cat /etc/passwd",
    "$(cat /etc/shadow)",
    "`whoami`",
])
def test_command_injection(cmd):
    result = execute_user_input(cmd)
    assert not result.executes_shell()
```

---

## G-PERF: Performance Anti-Patterns

### What It Looks Like
- String concatenation in loops
- Nested iterations where O(n) exists
- Wrong data structure for the task
- Unnecessary memory allocations

### Why LLMs Do This
LLMs optimize for correctness, not efficiency. They learn "working" patterns, not optimal ones.

### Test Strategy
1. **Complexity bounds**: Measure scaling behavior
2. **Memory profiling**: Check allocation patterns
3. **Performance budgets**: Set time limits for operations

### Test Patterns

```python
# Pattern 1: Complexity verification
def test_complexity_bound():
    """Verify algorithm is O(n) or better."""
    sizes = [100, 1000, 10000, 100000]
    times = []
    for n in sizes:
        start = time.time()
        process_data(generate_data(n))
        times.append(time.time() - start)
    
    # Check if time scales linearly
    ratios = [times[i+1]/times[i] for i in range(len(times)-1)]
    assert all(r < 15 for r in ratios), "O(n²) or worse detected"

# Pattern 2: Memory efficiency
def test_memory_efficiency():
    """Verify memory doesn't grow with input size."""
    import tracemalloc
    tracemalloc.start()
    
    process_data(generate_data(100))
    mem1 = tracemalloc.get_traced_memory()[0]
    
    process_data(generate_data(10000))
    mem2 = tracemalloc.get_traced_memory()[0]
    
    # Memory should not scale linearly for streaming ops
    assert mem2 < mem1 * 10, "Memory leak or inefficient allocation"

# Pattern 3: Performance budget
def test_performance_budget():
    """Verify operation completes within time budget."""
    BUDGET_MS = 100  # 100ms budget
    
    start = time.time()
    result = process_data(typical_input)
    elapsed_ms = (time.time() - start) * 1000
    
    assert elapsed_ms < BUDGET_MS, f"Took {elapsed_ms}ms, budget {BUDGET_MS}ms"
```

---

## G-ERR: Missing Error Handling

### What It Looks Like
- Try-catch blocks that just log
- No null/undefined checks
- Missing error boundaries
- Errors that crash instead of degrade

### Why LLMs Do This
LLMs see happy path examples more often in training data.

### Test Strategy
1. **Fault injection**: Deliberately cause errors
2. **Error path coverage**: Test every error branch
3. **Recovery verification**: Ensure graceful degradation

### Test Patterns

```python
# Pattern 1: Null/None handling
@pytest.mark.parametrize("input", [
    None,
    "",
    [],
    {},
    float('nan'),
    float('inf'),
])
def test_handles_none_empty(input):
    """Code should handle null-like inputs gracefully."""
    result = process(input)  # Should not crash
    assert result is not None or result == sentinel_value()

# Pattern 2: Exception propagation
def test_exception_handling():
    """Exceptions should be handled, not swallowed."""
    # Inject a failure
    with mock.patch('dependency.fetch', side_effect=ConnectionError):
        result = process()
        # Should handle gracefully, not crash
        assert result.status in ['error', 'partial', 'fallback']
        assert 'ConnectionError' not in str(result.error)

# Pattern 3: Error boundary test
def test_error_boundaries():
    """Verify error boundaries prevent cascade failures."""
    failures = inject_failures(process, ['db', 'api', 'cache'])
    for failure in failures:
        result = process_with_failure(failure)
        assert result.degraded_gracefully()
        assert not result.exposed_internals()

# Pattern 4: Recovery test
def test_error_recovery():
    """Verify system recovers from transient errors."""
    # First call fails
    with mock.patch('api.call', side_effect=TimeoutError):
        result1 = process()
        assert result1.status == 'error'
    
    # Second call should succeed (not stuck in error state)
    with mock.patch('api.call', return_value=good_response):
        result2 = process()
        assert result2.status == 'success'
```

---

## G-EDGE: Missing Edge Cases

### What It Looks Like
- No handling for empty arrays
- No handling for null values
- No handling for boundary values (MAX_INT, empty string)
- No handling for unicode/special characters

### Why LLMs Do This
Edge cases are underrepresented in training data.

### Test Strategy
1. **Edge case matrix**: Systematic coverage
2. **Property-based testing**: Let framework find edge cases
3. **Boundary testing**: Test exactly at boundaries

### Test Patterns

```python
# Pattern 1: Edge case matrix
@pytest.mark.parametrize("input,expected", [
    ([], []),                          # empty
    ([None], [default]),               # null element
    ([1], [1]),                        # single
    ([1, 2, 3], [1, 2, 3]),            # normal
    ([-MAX_INT, MAX_INT], handled),    # boundaries
    ([1, 1, 1], deduped),              # duplicates
    ([3, 2, 1], sorted_or_preserved),  # order matters
])
def test_edge_case_matrix(input, expected):
    result = process(input)
    assert result == expected

# Pattern 2: Property-based edge case discovery
from hypothesis import given, strategies as st

@given(st.lists(st.integers()))
def test_handles_any_list(items):
    """Should handle any list, including edge cases."""
    result = process(items)
    assert result is not None
    assert len(result) <= len(items)  # Or other invariant

@given(st.text(alphabet=st.characters(
    blacklist_categories=('Cs',)  # Skip surrogates
)))
def test_handles_any_string(text):
    """Should handle any valid string."""
    result = process(text)
    assert result is not None

# Pattern 3: Boundary testing
def test_boundaries():
    """Test exactly at boundaries."""
    boundaries = [
        0,
        1,
        -1,
        MAX_INT,
        MIN_INT,
        MAX_INT - 1,
        MIN_INT + 1,
    ]
    for b in boundaries:
        result = process(b)
        assert handles_correctly(result)
```

---

## G-DEP: Outdated Dependencies

### What It Looks Like
- Using deprecated APIs
- Installing old package versions
- Reintroducing fixed vulnerabilities

### Why LLMs Do This
Training data spans many years, mixing old and new patterns.

### Test Strategy
1. **Dependency scanning**: Check for known vulnerabilities
2. **Version auditing**: Ensure packages are current
3. **API deprecation check**: Verify APIs are still valid

### Test Patterns

```python
# Pattern 1: Version audit
def test_dependency_versions():
    """Verify dependencies are current."""
    deps = parse_requirements()
    for dep in deps:
        latest = fetch_latest_version(dep.name)
        age_days = (latest.release_date - dep.release_date).days
        assert age_days < 365, f"{dep.name} is {age_days} days old"

# Pattern 2: Vulnerability scan
def test_no_known_vulnerabilities():
    """Verify no dependencies have known CVEs."""
    deps = parse_requirements()
    for dep in deps:
        cves = fetch_cves(dep.name, dep.version)
        assert len(cves) == 0, f"{dep.name} has CVEs: {cves}"

# Pattern 3: Deprecation check
def test_no_deprecated_apis():
    """Verify no deprecated API usage."""
    deprecated_calls = extract_deprecated_usage(generated_code)
    assert len(deprecated_calls) == 0, f"Deprecated: {deprecated_calls}"
```

---

## G-CTX: Missing Context Dependencies

### What It Looks Like
- Code that works in isolation but fails integrated
- Missing environment variables
- Undocumented configuration requirements
- Cross-service dependencies not declared

### Why LLMs Do This
LLMs see partial code during generation, not full deployment context.

### Test Strategy
1. **Environment simulation**: Test with minimal env
2. **Dependency documentation**: Verify all deps documented
3. **Integration testing**: Test in realistic environment

### Test Patterns

```python
# Pattern 1: Environment completeness
def test_all_env_vars_documented():
    """Verify all environment variables are documented."""
    env_refs = extract_env_var_refs(generated_code)
    documented = parse_env_docs()
    for ref in env_refs:
        assert ref in documented, f"Undocumented: {ref}"

# Pattern 2: Minimal environment test
def test_works_with_minimal_env():
    """Should work with minimal configuration."""
    with clean_environment():
        with only_required_env_vars():
            result = process()
            assert result.succeeds_or_fails_gracefully()

# Pattern 3: Dependency injection test
def test_dependencies_injectable():
    """All external dependencies should be injectable."""
    with mock_injected_dependencies():
        result = process()
        assert result.uses_injected_deps()
```

---

## G-SEM: Semantic Errors

### What It Looks Like
- Code that compiles and runs but does the wrong thing
- Plausible logic that doesn't match requirements
- Correct syntax, incorrect semantics

### Why LLMs Do This
LLMs generate syntactically correct code that may not match the semantic intent.

### Test Strategy
1. **Behavioral testing**: Test actual behavior, not just output
2. **Specification matching**: Compare against formal spec
3. **Differential testing**: Compare against reference implementation

### Test Patterns

```python
# Pattern 1: Behavioral specification test
def test_matches_specification():
    """Behavior should match specification."""
    for case in specification.test_cases:
        result = generated_function(case.input)
        assert result.behavior_matches(case.expected_behavior)

# Pattern 2: Differential testing
def test_matches_reference():
    """Should match reference implementation."""
    test_inputs = generate_test_inputs()
    for inp in test_inputs:
        expected = reference_implementation(inp)
        actual = generated_implementation(inp)
        assert actual == expected

# Pattern 3: State machine testing
def test_state_transitions():
    """Verify correct state transitions."""
    states = initial_state()
    for action in action_sequence:
        states = apply_action(states, action)
        assert valid_state(states)
```
