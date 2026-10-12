use std::io;

use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};

pub(crate) struct Frame {
    pub bytes: Vec<u8>,
    pub oversized: bool,
    pub terminated: bool,
}

pub(crate) struct Lines<R> {
    reader: BufReader<R>,
    limit: usize,
    bytes: Vec<u8>,
    oversized: bool,
}

impl<R: AsyncRead + Unpin> Lines<R> {
    pub fn new(reader: R, limit: usize) -> Self {
        Self {
            reader: BufReader::new(reader),
            limit,
            bytes: Vec::new(),
            oversized: false,
        }
    }

    // The partial frame lives on self so selecting another branch cannot lose bytes.
    pub async fn next(&mut self) -> io::Result<Option<Frame>> {
        loop {
            let available = self.reader.fill_buf().await?;
            if available.is_empty() {
                if self.bytes.is_empty() && !self.oversized {
                    return Ok(None);
                }
                return Ok(Some(self.finish(false)));
            }
            let newline = available.iter().position(|byte| *byte == b'\n');
            let count = newline.unwrap_or(available.len());
            let remaining = self.limit.saturating_sub(self.bytes.len());
            let retained = remaining.min(count);
            self.bytes.extend_from_slice(&available[..retained]);
            self.oversized |= count > remaining;
            self.reader.consume(count + usize::from(newline.is_some()));
            if newline.is_some() {
                return Ok(Some(self.finish(true)));
            }
        }
    }

    fn finish(&mut self, terminated: bool) -> Frame {
        Frame {
            bytes: std::mem::take(&mut self.bytes),
            oversized: std::mem::take(&mut self.oversized),
            terminated,
        }
    }
}
