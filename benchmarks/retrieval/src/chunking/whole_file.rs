//! `whole_file` diagnostic control: one chunk per nonempty file.

use crate::chunking::{CHUNKER_VERSION, Chunk, Chunker, STRATEGY_WHOLE_FILE, chunk_id};
use crate::corpus::SourceFile;
use crate::{BenchResult, sha256_hex};

#[derive(Debug, Clone, Copy, Default)]
pub struct WholeFileChunker;

impl Chunker for WholeFileChunker {
    fn name(&self) -> &'static str {
        STRATEGY_WHOLE_FILE
    }

    fn config(&self) -> String {
        "control".to_string()
    }

    fn chunk(&self, file: &SourceFile) -> BenchResult<Vec<Chunk>> {
        if file.bytes.is_empty() {
            return Ok(Vec::new());
        }
        let end_byte = u32::try_from(file.bytes.len()).map_err(|_| crate::BenchError::Chunk {
            path: file.path.clone(),
            message: "file exceeds u32 byte range".to_string(),
        })?;
        let end_line = u32::try_from(file.line_count()).map_err(|_| crate::BenchError::Chunk {
            path: file.path.clone(),
            message: "file exceeds u32 line range".to_string(),
        })?;
        let config = self.config();
        Ok(vec![Chunk {
            path: file.path.clone(),
            start_byte: 0,
            end_byte,
            start_line: 1,
            end_line,
            text: file.text.clone(),
            strategy: STRATEGY_WHOLE_FILE.to_string(),
            version: CHUNKER_VERSION.to_string(),
            config: config.clone(),
            chunk_id: chunk_id(
                STRATEGY_WHOLE_FILE,
                CHUNKER_VERSION,
                &config,
                &file.path,
                0,
                end_byte,
                &sha256_hex(file.text.as_bytes()),
            ),
            fallback: false,
        }])
    }
}
