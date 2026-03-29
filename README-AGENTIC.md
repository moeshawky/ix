### AGENTIC README STANDARD V2.0: ix ARCHITECTURAL CONTRACT

## I. MISSION MANDATE
Ensure optimal search thinking spend for AI agents by providing high-signal retrieval of code primitives without context flooding and redundant environment maintenance.

## II. CORE PRINCIPLES
1. **Outcome Observation**: Verify existence via `-c` (count), locate via `-l` (files), and extract via `-C` (context).
2. **Sufficient Abstraction**: Search indices via the UTCP-compliant CLI. Freshness is managed automatically via the **Beacon Protocol**.
3. **Bounded Workspace**: Limit results via `-n` to prevent $C_t$ overflow. Constant-memory streaming ensures search safety on arbitrarily large files.

## III. REQUIRED STRUCTURE

### A. IDENTIFICATION AND HIGH-LEVEL CONTRACT

| Element | Constraint Mandate |
| :--- | :--- |
| **Name** | `ix` / `ixd` |
| **One-Liner** | Trigram-indexed high-performance code search for developers and agents. |
| **Dependencies** | Rust 2024, `libc`, `notify`, `crossbeam-channel`, `nix`. |
| **License** | MIT |

### B. FUNCTIONAL INTEGRATION

**Installation:**
```bash
cargo install --path . --features full
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
| `ix --force --build` | Force re-index | Bypasses Daemon authority |

### D. ARCHITECTURAL CONSTRAINTS & STATE

1. **Immutables**: Index stored in `.ix/shard.ix`. Format version 1.2. All posting lists are CRC32C-protected.
2. **Architectural Context**: `ix` is a service-aware consumer; `ixd` is the authoritative background producer (Daemon).
3. **Beacon Protocol**: Coordination plane via `.ix/beacon.json`. If a live beacon is found, CLI suppresses staleness warnings and defers to Daemon authority.
4. **Memory Constraint**: Search verification uses **constant memory** (streaming `read_line`) for all file types (regular, compressed, archived).

## IV. VALIDATION

*   **Success Criteria**: Exit code 0, zero redundant re-indexing loops, valid checksum verification.
*   **Outcome Verification**:
    ```bash
    ixd [PATH] &  # Start daemon
    ix "pattern"  # CLI should report "[ix] managed by ixd"
    ```
*   **Telemetry**: Use `--stats` flag for performance metrics and integrity status.
