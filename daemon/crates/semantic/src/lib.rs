//! Local embedding model (bge-small-en-v1.5 via ONNX Runtime).
//!
//! 384-dim, CLS-pooled, L2-normalized — cosine similarity between outputs is
//! meaningful, which is what the sqlite-vec index assumes.

pub mod embedder;
pub mod tagger;

pub use embedder::{Embedder, EMBEDDING_DIM};
pub use tagger::{TagResult, Tagger};

#[derive(Debug, thiserror::Error)]
pub enum SemanticError {
    #[error("model file missing: {0} (run build/fetch-models.sh)")]
    ModelMissing(std::path::PathBuf),
    #[error("tokenizer error: {0}")]
    Tokenizer(String),
    #[error("onnx runtime error: {0}")]
    Ort(#[from] ort::Error),
    #[error("llm error: {0}")]
    Llm(String),
}
