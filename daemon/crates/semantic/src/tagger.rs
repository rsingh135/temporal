//! Local LLM tag/summary generation via embedded llama.cpp (Metal).
//!
//! Runs asynchronously after a freeze: heuristic tags are the floor, this
//! enriches them. The model stays loaded (mmap-backed) in the daemon; each
//! generation gets a fresh inference context.

use std::num::NonZeroU32;
use std::path::Path;
use std::sync::OnceLock;

use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{AddBos, LlamaModel};
use llama_cpp_2::sampling::LlamaSampler;
use serde::Deserialize;
use tracing::info;

use crate::SemanticError;

const N_CTX: u32 = 4096;
const MAX_GENERATED_TOKENS: usize = 320;
/// Keep prompts comfortably inside N_CTX (~4 chars/token heuristic).
const MAX_CONTEXT_CHARS: usize = 9000;

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct TagResult {
    pub summary: String,
    pub tags: Vec<String>,
}

pub struct Tagger {
    model: LlamaModel,
}

fn backend() -> Result<&'static LlamaBackend, SemanticError> {
    static BACKEND: OnceLock<LlamaBackend> = OnceLock::new();
    if BACKEND.get().is_none() {
        let b = LlamaBackend::init().map_err(|e| SemanticError::Llm(e.to_string()))?;
        let _ = BACKEND.set(b);
    }
    Ok(BACKEND.get().expect("backend initialized above"))
}

impl Tagger {
    pub fn load(model_path: &Path) -> Result<Self, SemanticError> {
        if !model_path.exists() {
            return Err(SemanticError::ModelMissing(model_path.to_path_buf()));
        }
        let backend = backend()?;
        let params = LlamaModelParams::default().with_n_gpu_layers(1_000_000);
        let model = LlamaModel::load_from_file(backend, model_path, &params)
            .map_err(|e| SemanticError::Llm(e.to_string()))?;
        info!(model = %model_path.display(), "tagger loaded");
        Ok(Self { model })
    }

    /// Generates a short summary and keyword tags for a workspace context.
    pub fn generate(&self, context: &str) -> Result<TagResult, SemanticError> {
        let context: String = context.chars().take(MAX_CONTEXT_CHARS).collect();
        // Qwen3 ChatML; /no_think disables the thinking block.
        let prompt = format!(
            "<|im_start|>system\n\
             You label snapshots of a user's desktop. Reply with ONLY a JSON object: a one-sentence \
             summary (at most 14 words) naming the 2-4 MAIN projects or activities — never an \
             exhaustive list, never generic phrases like 'multiple projects' — and 5 to 10 short \
             lowercase keyword tags (concrete project names, technologies, topics).\n\
             Example reply: {{\"summary\": \"iOS work on the remy app with TestFlight uploads and \
             App Store Connect\", \"tags\": [\"remy-ios\", \"swift\", \"testflight\", \"app store\"]}} \
             /no_think<|im_end|>\n\
             <|im_start|>user\n{context}<|im_end|>\n\
             <|im_start|>assistant\n"
        );
        let raw = self.complete(&prompt)?;
        parse_tag_result(&raw)
            .ok_or_else(|| SemanticError::Llm(format!("model returned unparseable output: {raw}")))
    }

    fn complete(&self, prompt: &str) -> Result<String, SemanticError> {
        let llm = |e: &dyn std::fmt::Display| SemanticError::Llm(e.to_string());
        let backend = backend()?;
        let mut ctx = self
            .model
            .new_context(
                backend,
                LlamaContextParams::default()
                    .with_n_ctx(Some(NonZeroU32::new(N_CTX).expect("nonzero"))),
            )
            .map_err(|e| llm(&e))?;
        let tokens = self.model.str_to_token(prompt, AddBos::Always).map_err(|e| llm(&e))?;
        let n_batch = ctx.n_batch() as usize;

        let mut batch = LlamaBatch::new(n_batch, 1);
        let mut pos = 0i32;
        for chunk in tokens.chunks(n_batch) {
            batch.clear();
            for (i, &token) in chunk.iter().enumerate() {
                let is_last_of_prompt = pos as usize + i == tokens.len() - 1;
                batch.add(token, pos + i as i32, &[0], is_last_of_prompt).map_err(|e| llm(&e))?;
            }
            ctx.decode(&mut batch).map_err(|e| llm(&e))?;
            pos += chunk.len() as i32;
        }

        let mut sampler = LlamaSampler::greedy();
        // Accumulate raw piece bytes: a multi-byte UTF-8 character can be
        // split across tokens, so decoding per token would mangle it.
        let mut out_bytes: Vec<u8> = Vec::new();
        for _ in 0..MAX_GENERATED_TOKENS {
            let token = sampler.sample(&ctx, batch.n_tokens() - 1);
            if self.model.is_eog_token(token) {
                break;
            }
            out_bytes.extend(
                self.model.token_to_piece_bytes(token, 8, true, None).unwrap_or_default(),
            );
            batch.clear();
            batch.add(token, pos, &[0], true).map_err(|e| llm(&e))?;
            ctx.decode(&mut batch).map_err(|e| llm(&e))?;
            pos += 1;
        }
        Ok(String::from_utf8_lossy(&out_bytes).into_owned())
    }
}

/// Pulls the first JSON object out of the model output (which may include an
/// empty `<think>` block or stray prose around it).
fn parse_tag_result(raw: &str) -> Option<TagResult> {
    let start = raw.find('{')?;
    let end = raw.rfind('}')?;
    if end <= start {
        return None;
    }
    let parsed: TagResult = serde_json::from_str(&raw[start..=end]).ok()?;
    let tags: Vec<String> = parsed
        .tags
        .into_iter()
        .map(|t| t.trim().to_lowercase())
        .filter(|t| !t.is_empty())
        .take(12)
        .collect();
    // Small local models sometimes ignore length limits and dump the whole
    // context; a runaway "summary" is worse than the heuristic one.
    let words: Vec<&str> = parsed.summary.split_whitespace().collect();
    let summary = if words.is_empty() || words.len() > 24 {
        String::new() // caller keeps the heuristic summary
    } else {
        words.join(" ")
    };
    Some(TagResult { summary, tags })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_json_wrapped_in_think_block_and_prose() {
        let raw = "<think>\n</think>\nSure! {\"summary\": \" Rust daemon work \", \"tags\": [\"Rust\", \" ipc \", \"\"]} done";
        let result = parse_tag_result(raw).unwrap();
        assert_eq!(result.summary, "Rust daemon work");
        assert_eq!(result.tags, vec!["rust", "ipc"]);
    }

    #[test]
    fn rejects_output_without_json() {
        assert!(parse_tag_result("no json here").is_none());
    }
}
