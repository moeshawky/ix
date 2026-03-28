# CWE Security Patterns for LLM-Generated Code

## Table of Contents

1. [Priority CWEs](#priority-cwes)
2. [Language-Specific Detection](#language-specific-detection)
3. [LLM Self-Correction Failure Rates](#llm-self-correction-failure-rates)

## Priority CWEs

LLM code generators systematically produce these vulnerability classes [4][6][7]. 40% of LLM code suggestions in relevant contexts contain security bugs.

| CWE | Name | LLM Pattern | Detection |
|-----|------|-------------|-----------|
| CWE-476 | NULL Pointer Dereference | Omits NULL checks on function args | Grep for `unwrap()`, unchecked `.get()`, raw pointer deref |
| CWE-787 | Out-of-bounds Write | Defaults to unbounded string ops | Check `sprintf` vs `snprintf`, unchecked array indexing |
| CWE-416 | Use After Free | Incorrect lifetime/ownership reasoning | Rust: check `unsafe` blocks; C: grep for use-after-free patterns |
| CWE-119 | Buffer Overflow | Copies without size validation | Check `memcpy`/`strcpy` without length bounds |
| CWE-400 | Uncontrolled Resource Consumption | No limits on allocations | Check for unbounded `Vec::new()`, missing timeouts, no pagination |
| CWE-190 | Integer Overflow | Arithmetic without overflow checks | Look for `as usize` casts, unchecked arithmetic on user input |
| CWE-94 | Code Injection | 0% LLM self-fix rate | String interpolation in eval/exec/shell contexts |
| CWE-78 | OS Command Injection | 0% LLM self-fix rate | `format!()` or f-strings passed to `Command::new()` |
| CWE-20 | Input Validation | 0% LLM self-fix rate, needs human | Missing validation at system boundaries |

## Language-Specific Detection

### Rust
```
# Unsafe unwrap (CWE-476 equivalent)
grep -rn '\.unwrap()' --include='*.rs' | grep -v '#\[test\]' | grep -v 'test_'

# Unchecked cast (CWE-190)
grep -rn 'as usize\|as u32\|as i32' --include='*.rs'

# Command injection (CWE-78)
grep -rn 'Command::new.*format!' --include='*.rs'

# Unbounded allocation (CWE-400)
grep -rn 'Vec::with_capacity.*user\|read_to_string' --include='*.rs'
```

### Python
```
# Command injection (CWE-78)
grep -rn 'subprocess.*shell=True\|os.system\|os.popen' --include='*.py'

# Code injection (CWE-94)
grep -rn 'eval(\|exec(' --include='*.py'

# SQL injection (CWE-89)
grep -rn 'f".*SELECT\|format.*SELECT\|%.*SELECT' --include='*.py'
```

### TypeScript/JavaScript
```
# XSS (CWE-79)
grep -rn 'innerHTML\|dangerouslySetInnerHTML\|document.write' --include='*.ts' --include='*.tsx'

# Prototype pollution (CWE-1321)
grep -rn 'Object.assign.*req\.\|\.\.\.req\.' --include='*.ts'
```

## LLM Self-Correction Failure Rates

From [4][6]: when asked to fix their own security bugs, LLMs fail at these rates:

| Vulnerability | Self-Fix Rate | Implication |
|---------------|---------------|-------------|
| CWE-94 (Code Injection) | 0% | Always requires human review |
| CWE-78 (OS Command Injection) | 0% | Always requires human review |
| CWE-20 (Input Validation) | 0% | Always requires human review |
| CWE-476 (NULL Pointer) | ~40% | Partial, verify manually |
| CWE-787 (OOB Write) | ~35% | Partial, verify manually |

**Rule:** For CWE-94, CWE-78, and CWE-20, NEVER accept an LLM's self-correction. Require human review.
