# About ix

`ix` is a high-performance, Unix-native search engine that eliminates the linear scan bottleneck of traditional tools. By leveraging a sparse trigram index and a constant-memory streaming architecture, `ix` delivers sub-millisecond retrieval across multi-gigabyte codebases. 

Engineered for the 2026 workflow, it provides high-signal code retrieval for both human developers and AI agents, seamlessly handling compressed files, archives, and piped input.

## Why ix?

Traditional search tools like `grep` and `ripgrep` are exceptionally fast, but they are fundamentally limited by the linear scanning of files on disk ($O(n)$). As codebases grow into the gigabyte range, even the most optimized linear scanner begins to introduce latency that breaks the "flow" of development and causes context-window flooding for AI agents.

`ix` solves this by:
1. **Indexing**: Pre-computing a byte-level trigram index to narrow search candidates to a fraction of the total file set.
2. **Streaming**: Verifying candidate matches using a memory-constant streaming architecture, allowing it to search massive files without OOM risks.
3. **Agent-First Design**: Providing structured output and precise context extraction out of the box.
