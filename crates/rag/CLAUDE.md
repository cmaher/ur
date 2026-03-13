# rag (RAG Library)

Library crate providing `RagManager` for document indexing and semantic search via Qdrant + fastembed.

- `RagManager` implements `Clone` and accepts dependencies (Qdrant client, `Arc<TextEmbedding>`) via constructor (DI)
- Per-language Qdrant collections: `rag_docs_rust`, etc. (384-dim vectors, cosine distance)
- Indexing reads markdown from a docs dir, chunks with `text-splitter`, embeds with fastembed, upserts to Qdrant
- Search embeds query, searches Qdrant with language filter, returns top-K results
- No proto types defined here — uses `ur_rpc::proto::rag::*` for the `Language` enum and response types
- Chunking and embedding are CPU-intensive; fastembed model is shared via `Arc<TextEmbedding>`
