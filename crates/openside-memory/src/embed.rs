//! Local static (model2vec-style) embeddings: a WordPiece tokenizer plus one
//! `[vocab, dim]` matrix, mean-pooled and L2-normalized. Loading reads two files
//! from a local directory and never touches the network.

use std::{collections::HashMap, fmt, path::Path};

use serde_json::Value;
use sha2::{Digest, Sha256};

/// Hugging Face `tokenizers` JSON; only the `WordPiece` model type is supported.
pub const TOKENIZER_FILE: &str = "tokenizer.json";
/// Safetensors file holding an `embeddings` (or the only 2-D) tensor in F32/F16/BF16.
pub const MODEL_FILE: &str = "model.safetensors";

/// Text-to-vector function used for semantic recall.
pub trait Embedder: Send + Sync {
    /// Stable identifier; stored vectors from another identifier are recomputed.
    fn id(&self) -> &str;
    /// Unit-length vector, or none when the text has no known token.
    fn embed(&self, text: &str) -> Option<Vec<f32>>;
}

/// Why a local model directory cannot be used; never contains file contents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelError(pub String);

impl fmt::Display for ModelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ModelError {}

fn invalid(message: impl Into<String>) -> ModelError {
    ModelError(message.into())
}

/// Static token-embedding model loaded from a local directory.
pub struct StaticEmbedder {
    id: String,
    vocab: HashMap<String, u32>,
    unk: Option<u32>,
    prefix: String,
    lowercase: bool,
    max_chars: usize,
    rows: usize,
    dim: usize,
    matrix: Vec<f32>,
}

impl fmt::Debug for StaticEmbedder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StaticEmbedder")
            .field("id", &self.id)
            .field("rows", &self.rows)
            .field("dim", &self.dim)
            .finish()
    }
}

impl StaticEmbedder {
    /// Load [`TOKENIZER_FILE`] and [`MODEL_FILE`] from `dir`. `dir` is resolved once (it may
    /// be the symlink that `openside memory model fetch` swaps between immutable
    /// generations), so both files always come from the same generation.
    pub fn load(dir: &Path) -> Result<Self, ModelError> {
        let resolved = std::fs::canonicalize(dir).map_err(|err| {
            invalid(format!(
                "{TOKENIZER_FILE} cannot be read in {} ({})",
                dir.display(),
                err.kind()
            ))
        })?;
        let read = |name: &str| {
            std::fs::read(resolved.join(name)).map_err(|err| {
                invalid(format!(
                    "{} cannot be read in {} ({})",
                    name,
                    dir.display(),
                    err.kind()
                ))
            })
        };
        let tokenizer_bytes = read(TOKENIZER_FILE)?;
        let model_bytes = read(MODEL_FILE)?;
        let tokenizer: Value = serde_json::from_slice(&tokenizer_bytes)
            .map_err(|_| invalid(format!("{TOKENIZER_FILE} is not valid JSON")))?;
        let model = &tokenizer["model"];
        if model["type"].as_str() != Some("WordPiece") {
            return Err(invalid(format!(
                "{TOKENIZER_FILE} uses an unsupported tokenizer model; only WordPiece is supported"
            )));
        }
        // Only transformations this tokenizer reproduces are accepted; anything else would
        // silently produce token ids that differ from the model's training.
        let kind = |section: &str| tokenizer[section]["type"].as_str().map(str::to_owned);
        match kind("normalizer").as_deref() {
            None | Some("BertNormalizer" | "Lowercase") => {}
            Some(other) => {
                return Err(invalid(format!(
                    "{TOKENIZER_FILE} normalizer {other} is not supported"
                )))
            }
        }
        let normalizer = &tokenizer["normalizer"];
        if normalizer["strip_accents"].as_bool() == Some(true) {
            return Err(invalid(format!(
                "{TOKENIZER_FILE} normalizer option strip_accents is not supported"
            )));
        }
        if normalizer["handle_chinese_chars"].as_bool() == Some(false) {
            return Err(invalid(format!(
                "{TOKENIZER_FILE} normalizer option handle_chinese_chars=false is not supported"
            )));
        }
        match kind("pre_tokenizer").as_deref() {
            None | Some("BertPreTokenizer") => {}
            Some(other) => {
                return Err(invalid(format!(
                    "{TOKENIZER_FILE} pre-tokenizer {other} is not supported"
                )))
            }
        }
        let vocab: HashMap<String, u32> = model["vocab"]
            .as_object()
            .ok_or_else(|| invalid(format!("{TOKENIZER_FILE} has no WordPiece vocabulary")))?
            .iter()
            .filter_map(|(token, id)| Some((token.clone(), u32::try_from(id.as_u64()?).ok()?)))
            .collect();
        let unk_token = model["unk_token"].as_str().unwrap_or("[UNK]");
        let (rows, dim, matrix) = load_matrix(&model_bytes)?;
        let mut hash = Sha256::new();
        hash.update(&tokenizer_bytes);
        hash.update(&model_bytes);
        let digest = hash.finalize();
        let id = format!(
            "static-{}",
            digest[..8]
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        );
        Ok(Self {
            id,
            unk: vocab.get(unk_token).copied(),
            vocab,
            prefix: model["continuing_subword_prefix"]
                .as_str()
                .unwrap_or("##")
                .to_owned(),
            lowercase: lowercases(&tokenizer["normalizer"]),
            max_chars: model["max_input_chars_per_word"]
                .as_u64()
                .and_then(|value| usize::try_from(value).ok())
                .unwrap_or(100),
            rows,
            dim,
            matrix,
        })
    }

    /// Embedding dimension.
    pub fn dim(&self) -> usize {
        self.dim
    }

    fn tokens(&self, text: &str) -> Vec<u32> {
        let text = if self.lowercase {
            text.to_lowercase()
        } else {
            text.to_owned()
        };
        let mut ids = Vec::new();
        for word in pre_tokenize(&text) {
            self.word_piece(word, &mut ids);
        }
        ids
    }

    fn word_piece(&self, word: &str, ids: &mut Vec<u32>) {
        let chars: Vec<(usize, char)> = word.char_indices().collect();
        if chars.len() > self.max_chars {
            ids.extend(self.unk);
            return;
        }
        let mut pieces = Vec::new();
        let mut start = 0;
        while start < chars.len() {
            let mut end = chars.len();
            let mut found = None;
            while end > start {
                let from = chars[start].0;
                let to = chars.get(end).map_or(word.len(), |(offset, _)| *offset);
                let piece = &word[from..to];
                let id = if start > 0 {
                    self.vocab.get(&format!("{}{piece}", self.prefix))
                } else {
                    self.vocab.get(piece)
                };
                if let Some(id) = id {
                    found = Some(*id);
                    break;
                }
                end -= 1;
            }
            match found {
                Some(id) => pieces.push(id),
                None => {
                    ids.extend(self.unk);
                    return;
                }
            }
            start = end;
        }
        ids.extend(pieces);
    }
}

impl Embedder for StaticEmbedder {
    fn id(&self) -> &str {
        &self.id
    }

    fn embed(&self, text: &str) -> Option<Vec<f32>> {
        let mut sum = vec![0f32; self.dim];
        let mut count = 0usize;
        for id in self.tokens(text) {
            // model2vec excludes the unknown token from pooling.
            if Some(id) == self.unk {
                continue;
            }
            let row = id as usize;
            if row >= self.rows {
                continue;
            }
            for (acc, value) in sum
                .iter_mut()
                .zip(&self.matrix[row * self.dim..(row + 1) * self.dim])
            {
                *acc += value;
            }
            count += 1;
        }
        if count == 0 {
            return None;
        }
        let norm = sum.iter().map(|value| value * value).sum::<f32>().sqrt();
        if !norm.is_normal() {
            return None;
        }
        for value in &mut sum {
            *value /= norm;
        }
        Some(sum)
    }
}

fn lowercases(normalizer: &Value) -> bool {
    match normalizer["type"].as_str() {
        Some("BertNormalizer") => normalizer["lowercase"].as_bool().unwrap_or(true),
        Some("Lowercase") => true,
        Some("Sequence") => normalizer["normalizers"]
            .as_array()
            .is_some_and(|items| items.iter().any(lowercases)),
        _ => false,
    }
}

fn is_cjk(c: char) -> bool {
    matches!(u32::from(c),
        0x4E00..=0x9FFF | 0x3400..=0x4DBF | 0x20000..=0x2A6DF | 0x2A700..=0x2B73F
        | 0x2B740..=0x2B81F | 0x2B820..=0x2CEAF | 0xF900..=0xFAFF | 0x2F800..=0x2FA1F)
}

/// BERT pre-tokenization: whitespace splits, punctuation and CJK ideographs stand alone.
fn pre_tokenize(text: &str) -> Vec<&str> {
    let mut words = Vec::new();
    let mut start = None;
    for (offset, c) in text.char_indices() {
        let single = is_cjk(c) || (!c.is_alphanumeric() && !c.is_whitespace() && !c.is_control());
        if c.is_whitespace() || c.is_control() || single {
            if let Some(begin) = start.take() {
                words.push(&text[begin..offset]);
            }
            if single {
                words.push(&text[offset..offset + c.len_utf8()]);
            }
        } else if start.is_none() {
            start = Some(offset);
        }
    }
    if let Some(begin) = start {
        words.push(&text[begin..]);
    }
    words
}

fn load_matrix(bytes: &[u8]) -> Result<(usize, usize, Vec<f32>), ModelError> {
    let bad = || invalid(format!("{MODEL_FILE} is not a valid safetensors file"));
    let header_len = bytes
        .get(..8)
        .and_then(|raw| raw.try_into().ok())
        .map(u64::from_le_bytes)
        .and_then(|len| usize::try_from(len).ok())
        .ok_or_else(bad)?;
    let header_end = header_len.checked_add(8).ok_or_else(bad)?;
    let header: serde_json::Map<String, Value> =
        serde_json::from_slice(bytes.get(8..header_end).ok_or_else(bad)?).map_err(|_| bad())?;
    let data = &bytes[header_end..];
    let two_d: Vec<(&String, &Value)> = header
        .iter()
        .filter(|(name, info)| {
            name.as_str() != "__metadata__"
                && info["shape"]
                    .as_array()
                    .is_some_and(|shape| shape.len() == 2)
        })
        .collect();
    let (_, info) = two_d
        .iter()
        .find(|(name, _)| name.as_str() == "embeddings")
        .or_else(|| (two_d.len() == 1).then(|| &two_d[0]))
        .ok_or_else(|| {
            invalid(format!(
                "{MODEL_FILE} has no `embeddings` tensor (or single 2-D tensor)"
            ))
        })?;
    let dimension = |index: usize| {
        info["shape"][index]
            .as_u64()
            .and_then(|value| usize::try_from(value).ok())
            .ok_or_else(bad)
    };
    let (rows, dim) = (dimension(0)?, dimension(1)?);
    let offset = |index: usize| {
        info["data_offsets"][index]
            .as_u64()
            .and_then(|value| usize::try_from(value).ok())
            .ok_or_else(bad)
    };
    let raw = data.get(offset(0)?..offset(1)?).ok_or_else(bad)?;
    let width = match info["dtype"].as_str() {
        Some("F32") => 4,
        Some("F16" | "BF16") => 2,
        _ => {
            return Err(invalid(format!(
                "{MODEL_FILE} tensor dtype must be F32, F16 or BF16"
            )))
        }
    };
    if dim == 0 || rows.checked_mul(dim).and_then(|n| n.checked_mul(width)) != Some(raw.len()) {
        return Err(bad());
    }
    let matrix = match info["dtype"].as_str() {
        Some("F32") => raw
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect(),
        Some("F16") => raw
            .chunks_exact(2)
            .map(|c| f16_to_f32(u16::from_le_bytes([c[0], c[1]])))
            .collect(),
        _ => raw
            .chunks_exact(2)
            .map(|c| f32::from_bits(u32::from(u16::from_le_bytes([c[0], c[1]])) << 16))
            .collect(),
    };
    Ok((rows, dim, matrix))
}

fn f16_to_f32(half: u16) -> f32 {
    let sign = u32::from(half >> 15) << 31;
    let exponent = u32::from((half >> 10) & 0x1f);
    let mantissa = u32::from(half & 0x3ff);
    let bits = match (exponent, mantissa) {
        (0, 0) => sign,
        (0, _) => {
            let mut exponent = 113u32;
            let mut mantissa = mantissa;
            while mantissa & 0x400 == 0 {
                mantissa <<= 1;
                exponent -= 1;
            }
            sign | (exponent << 23) | ((mantissa & 0x3ff) << 13)
        }
        (0x1f, _) => sign | 0x7f80_0000 | (mantissa << 13),
        _ => sign | ((exponent + 112) << 23) | (mantissa << 13),
    };
    f32::from_bits(bits)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn half_precision_conversion_matches_reference_values() {
        assert_eq!(f16_to_f32(0x3c00), 1.0);
        assert_eq!(f16_to_f32(0xc000), -2.0);
        assert_eq!(f16_to_f32(0x3555), 0.333_251_95);
        assert_eq!(f16_to_f32(0x0001), 5.960_464_5e-8);
        assert!(f16_to_f32(0x7c00).is_infinite());
    }

    #[test]
    fn pre_tokenizer_splits_punctuation_and_cjk() {
        assert_eq!(
            pre_tokenize("Hello, world!  漢字 ok"),
            vec!["Hello", ",", "world", "!", "漢", "字", "ok"]
        );
    }

    #[test]
    fn word_piece_matches_the_bert_reference_splits() {
        let vocab: HashMap<String, u32> =
            ["[UNK]", "un", "##aff", "##able", "runn", "##ing", ",", "漢"]
                .iter()
                .enumerate()
                .map(|(id, token)| ((*token).to_owned(), u32::try_from(id).unwrap()))
                .collect();
        let embedder = StaticEmbedder {
            id: "reference".into(),
            unk: Some(0),
            vocab,
            prefix: "##".into(),
            lowercase: true,
            max_chars: 100,
            rows: 8,
            dim: 1,
            matrix: vec![0.0; 8],
        };
        // Google BERT reference: "unaffable" -> un ##aff ##able; an unsplittable word is one [UNK].
        assert_eq!(
            embedder.tokens("UnAffable, running 漢 xyz"),
            vec![1, 2, 3, 6, 4, 5, 7, 0]
        );
    }
}
