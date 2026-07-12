use std::path::Path;

use ort::session::builder::GraphOptimizationLevel;
use ort::session::Session;
use ort::value::Tensor;
use tokenizers::Tokenizer;
use tracing::info;

use crate::SemanticError;

pub const EMBEDDING_DIM: usize = 384;
const MAX_TOKENS: usize = 512;

/// bge-small-en-v1.5 asymmetric retrieval: queries get this instruction
/// prefix, documents are embedded raw.
const QUERY_PREFIX: &str = "Represent this sentence for searching relevant passages: ";

pub struct Embedder {
    session: Session,
    tokenizer: Tokenizer,
}

impl Embedder {
    /// Loads from a directory containing `model.onnx` and `tokenizer.json`.
    pub fn load(model_dir: &Path) -> Result<Self, SemanticError> {
        let model_path = model_dir.join("model.onnx");
        let tokenizer_path = model_dir.join("tokenizer.json");
        for path in [&model_path, &tokenizer_path] {
            if !path.exists() {
                return Err(SemanticError::ModelMissing(path.clone()));
            }
        }
        let mut tokenizer = Tokenizer::from_file(&tokenizer_path)
            .map_err(|e| SemanticError::Tokenizer(e.to_string()))?;
        tokenizer
            .with_truncation(Some(tokenizers::TruncationParams {
                max_length: MAX_TOKENS,
                ..Default::default()
            }))
            .map_err(|e| SemanticError::Tokenizer(e.to_string()))?;
        let session = Session::builder()?
            .with_optimization_level(GraphOptimizationLevel::Level3)?
            .commit_from_file(&model_path)?;
        info!(model = %model_path.display(), "embedder loaded");
        Ok(Self { session, tokenizer })
    }

    pub fn embed_document(&mut self, text: &str) -> Result<Vec<f32>, SemanticError> {
        self.embed(text)
    }

    pub fn embed_query(&mut self, text: &str) -> Result<Vec<f32>, SemanticError> {
        self.embed(&format!("{QUERY_PREFIX}{text}"))
    }

    fn embed(&mut self, text: &str) -> Result<Vec<f32>, SemanticError> {
        let encoding = self
            .tokenizer
            .encode(text, true)
            .map_err(|e| SemanticError::Tokenizer(e.to_string()))?;
        let len = encoding.get_ids().len();
        let ids: Vec<i64> = encoding.get_ids().iter().map(|&x| x as i64).collect();
        let mask: Vec<i64> = encoding.get_attention_mask().iter().map(|&x| x as i64).collect();
        let type_ids: Vec<i64> = encoding.get_type_ids().iter().map(|&x| x as i64).collect();

        let outputs = self.session.run(ort::inputs![
            "input_ids" => Tensor::from_array(([1usize, len], ids))?,
            "attention_mask" => Tensor::from_array(([1usize, len], mask))?,
            "token_type_ids" => Tensor::from_array(([1usize, len], type_ids))?,
        ])?;

        // last_hidden_state: [1, len, 384]; BGE pools the CLS (first) token.
        let (shape, data) = outputs[0].try_extract_tensor::<f32>()?;
        let hidden = shape[2] as usize;
        debug_assert_eq!(hidden, EMBEDDING_DIM);
        let cls = &data[..hidden];

        let norm = cls.iter().map(|v| v * v).sum::<f32>().sqrt().max(1e-12);
        Ok(cls.iter().map(|v| v / norm).collect())
    }
}
