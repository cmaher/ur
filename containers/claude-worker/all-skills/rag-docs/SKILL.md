---
name: rag-docs
description: Use when needing to look up documentation for any dependency, library, crate, or API — before guessing at usage, types, or function signatures
---

# RAG Documentation Search

## Overview

Search indexed dependency documentation using semantic search. **Always use this before guessing at API usage, types, or function signatures.**

## When to Use

- Looking up how to use a crate/library API
- Checking function signatures, types, or trait implementations
- Designing a feature that depends on an external library
- Unsure about correct usage patterns for a dependency
- Reviewing code that uses unfamiliar APIs

## Command

```bash
ur-tools rag search "<query>" [--language rust] [--top-k 5]
```

- `--language`: Documentation language (default: `rust`)
- `--top-k`: Number of results (default: `5`), increase for broad queries

## Examples

```bash
ur-tools rag search "qdrant create collection cosine distance"
ur-tools rag search "tonic gRPC streaming response"
ur-tools rag search "fastembed embedding model" --top-k 10
```

## Tips

- Use specific terms: function names, type names, error messages
- Broader queries benefit from higher `--top-k`
- Results include source file paths — read the full file if a chunk looks relevant
- If results are empty, the docs may not be indexed yet; fall back to reading `Cargo.toml` and source code
