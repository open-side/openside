//! Tiny synthetic static-embedding model generated on disk (no download).
//!
//! Four concept axes: vehicle (car/automobile/vehicle), parking (parked/garage/parking),
//! cat (cat/kitten), rest (sleeps/sofa/couch). Function words have zero vectors, so a
//! paraphrase that shares no content word with a memory still shares its concepts.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;

use serde_json::{json, Map, Value};

const WORDS: &[(&str, [f32; 4])] = &[
    ("[PAD]", [0.0; 4]),
    ("[UNK]", [0.0; 4]),
    ("the", [0.0; 4]),
    ("is", [0.0; 4]),
    ("in", [0.0; 4]),
    ("on", [0.0; 4]),
    ("my", [0.0; 4]),
    ("where", [0.0; 4]),
    ("car", [1.0, 0.0, 0.0, 0.0]),
    ("automobile", [1.0, 0.0, 0.0, 0.0]),
    ("vehicle", [0.9, 0.1, 0.0, 0.0]),
    ("parked", [0.0, 1.0, 0.0, 0.0]),
    ("parking", [0.0, 1.0, 0.0, 0.0]),
    ("garage", [0.1, 0.9, 0.0, 0.0]),
    ("cat", [0.0, 0.0, 1.0, 0.0]),
    ("kitten", [0.0, 0.0, 1.0, 0.0]),
    ("sleeps", [0.0, 0.0, 0.0, 1.0]),
    ("sofa", [0.0, 0.0, 0.0, 1.0]),
    ("couch", [0.0, 0.0, 0.0, 1.0]),
    ("auto", [1.0, 0.0, 0.0, 0.0]),
    ("##mobile", [1.0, 0.0, 0.0, 0.0]),
];

/// Write `tokenizer.json` + `model.safetensors` (dtype `F32` or `F16`) into `dir`.
pub fn write_model(dir: &Path, dtype: &str) {
    std::fs::create_dir_all(dir).unwrap();
    let vocab: Map<String, Value> = WORDS
        .iter()
        .enumerate()
        .map(|(id, (word, _))| ((*word).to_owned(), json!(id)))
        .collect();
    let tokenizer = json!({
        "version": "1.0",
        "normalizer": {"type": "BertNormalizer", "lowercase": true},
        "pre_tokenizer": {"type": "BertPreTokenizer"},
        "model": {
            "type": "WordPiece",
            "unk_token": "[UNK]",
            "continuing_subword_prefix": "##",
            "max_input_chars_per_word": 100,
            "vocab": vocab,
        }
    });
    std::fs::write(
        dir.join(openside_memory::TOKENIZER_FILE),
        serde_json::to_vec(&tokenizer).unwrap(),
    )
    .unwrap();
    let mut data = Vec::new();
    for (_, row) in WORDS {
        for value in row {
            match dtype {
                "F32" => data.extend(value.to_le_bytes()),
                "F16" => data.extend(f32_to_f16(*value).to_le_bytes()),
                _ => panic!("unsupported fixture dtype"),
            }
        }
    }
    let mut header = serde_json::to_vec(&json!({
        "__metadata__": {"format": "pt"},
        "embeddings": {"dtype": dtype, "shape": [WORDS.len(), 4], "data_offsets": [0, data.len()]}
    }))
    .unwrap();
    while header.len() % 8 != 0 {
        header.push(b' ');
    }
    let mut file = (header.len() as u64).to_le_bytes().to_vec();
    file.extend(header);
    file.extend(data);
    std::fs::write(dir.join(openside_memory::MODEL_FILE), file).unwrap();
}

// Fixture values are 0, 0.1, 0.9 and 1: normal-range conversion is enough.
fn f32_to_f16(value: f32) -> u16 {
    if value == 0.0 {
        return 0;
    }
    let bits = value.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exponent = ((bits >> 23) & 0xff) as i32 - 127 + 15;
    let mantissa = ((bits >> 13) & 0x3ff) as u16;
    sign | ((exponent as u16) << 10) | mantissa
}
