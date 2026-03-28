### AGENTIC README STANDARD V2.0: ix ARCHITECTURAL CONTRACT

## I. MISSION MANDATE
Ensure optimal search thinking spend for AI agents by providing high-signal retrieval of code primitives without context flooding.

## II. CORE PRINCIPLES
1. **Outcome Observation**: Verify existence via `-c` (count), locate via `-l` (files), and extract via `-C` (context).
2. **Sufficient Abstraction**: Search indices via the UTCP-compliant CLI without manual trigram extraction.
3. **Bounded Workspace**: Limit results via `-n` to prevent $C_t$ overflow.

## III. REQUIRED STRUCTURE

### A. IDENTIFICATION AND HIGH-LEVEL CONTRACT

| Element | Constraint Mandate |
| :--- | :--- |
| **Name** | `ix` / `ixd` |
| **One-Liner** | Trigram-indexed high-performance code search for developers and agents. |
| **Dependencies** | Rust 2024, `libc`, `notify`, `crossbeam-channel`. |
| **License** | MIT |

### B. FUNCTIONAL INTEGRATION

**Installation:**
```bash
cargo install --path .
```

**Index Initialization:**
```bash
ix --build [PATH]
```

**Search Execution:**
```bash
ix [FLAGS] "pattern" [PATH]
```

### C. UTCP API SPECIFICATION (CLI Schema)

| Command | Action | Agent Output |
| :--- | :--- | :--- |
| `ix -c "p"` | Count matches | Single integer |
| `ix -l "p"` | List files | Unique file paths |
| `ix --json "p"` | Structured extraction | JSON Lines (Schema: `{file, line, col, content, byte_offset}`) |
| `ix -C N "p"` | Contextual retrieval | ±N lines around match |
| `ix --fresh "p"` | Atomic rebuild + search | Guaranteed up-to-date results |

### D. ARCHITECTURAL CONSTRAINTS & STATE

1. **Immutables**: Index stored in `.ix/shard.ix`. Format version 1.1.
2. **Architectural Context**: `ix` is a stateless consumer; `ixd` is a stateful background producer (Daemon).
3. **Context Momentum**: The daemon tracks `IdleTracker` state (Active/Idle/Dormant) to optimize resource consumption.

## IV. VALIDATION

*   **Success Criteria**: Exit code 0, non-empty matches for existing patterns, valid JSON output.
*   **Outcome Verification**:
    ```bash
    ix --build && ix "pattern"
    ```
*   **Telemetry**: Use `--stats` flag for performance and retrieval metrics.
