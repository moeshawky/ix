# About `ix`

`ix` is a Unix-native code search tool designed for developers who demand both speed and simplicity. 

While traditional search tools like `grep` and `ripgrep` are exceptionally fast, they are fundamentally limited by the linear scanning of files on disk. As codebases grow into the gigabytes, even the fastest scanner introduces latency that breaks a developer's flow.

### The Trigram Advantage

`ix` solves this by building a compact, byte-level trigram index. By decomposing every file into overlapping 3-byte sequences, `ix` creates a map of the entire codebase. When you search for a pattern, `ix` consults this index to instantly identify only the files that *could* contain your match.

### Key Philosophies

*   **Unix Native**: Designed to be a drop-in replacement or supplement to your existing CLI pipelines. It speaks the language of `stdin`, `stdout`, and standard file-line formatting.
*   **Performance First**: Built in Rust with zero-copy `mmap` access and Bloom filter optimizations, ensuring search time scales with the number of matches, not the size of the repository.
*   **Byte-Level Correctness**: By operating on raw bytes rather than high-level abstractions, `ix` is inherently robust across different encodings and file types without the overhead of complex charset detection.
*   **Dormancy-Aware**: Future integration with the `ixd` daemon ensures your index stays fresh by utilizing system idle time, respecting your primary development work.

`ix` isn't just a search tool; it's a productivity multiplier for the modern, massive-scale development environment.
