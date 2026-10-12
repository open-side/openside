#[derive(Default)]
pub(crate) struct Decoder {
    line: Vec<u8>,
    data: Vec<u8>,
    id: Option<u64>,
    after_cr: bool,
    first_line: bool,
    started: bool,
}

pub(crate) struct Record {
    pub(crate) data: Vec<u8>,
    pub(crate) id: Option<u64>,
}

impl Decoder {
    pub(crate) fn push(&mut self, bytes: &[u8]) -> Result<Vec<Record>, String> {
        let mut records = Vec::new();
        for byte in bytes {
            if self.after_cr {
                self.after_cr = false;
                if *byte == b'\n' {
                    continue;
                }
            }
            if *byte == b'\r' || *byte == b'\n' {
                self.finish_line(&mut records)?;
                self.after_cr = *byte == b'\r';
            } else {
                self.line.push(*byte);
            }
        }
        Ok(records)
    }

    fn finish_line(&mut self, records: &mut Vec<Record>) -> Result<(), String> {
        if !self.started {
            self.first_line = true;
            self.started = true;
        }
        let mut line = std::mem::take(&mut self.line);
        if self.first_line {
            self.first_line = false;
            if line.starts_with(&[0xef, 0xbb, 0xbf]) {
                line.drain(..3);
            }
        }
        if line.is_empty() {
            if !self.data.is_empty() {
                self.data.pop();
                records.push(Record {
                    data: std::mem::take(&mut self.data),
                    id: self.id,
                });
            }
            self.line = line;
            return Ok(());
        }
        if !line.starts_with(b":") {
            let colon = line.iter().position(|byte| *byte == b':');
            let (field, mut value) = match colon {
                Some(colon) => (&line[..colon], &line[colon + 1..]),
                None => (line.as_slice(), &[][..]),
            };
            if value.starts_with(b" ") {
                value = &value[1..];
            }
            match field {
                b"data" => {
                    self.data.extend_from_slice(value);
                    self.data.push(b'\n');
                }
                b"id" if !value.contains(&0) => {
                    self.id = if value.is_empty() {
                        None
                    } else {
                        Some(
                            std::str::from_utf8(value)
                                .ok()
                                .and_then(|value| value.parse().ok())
                                .ok_or("SSE id is not a decimal sequence")?,
                        )
                    };
                }
                _ => {}
            }
        }
        line.clear();
        self.line = line;
        Ok(())
    }
}
