/// Minimal SSE decoder: one `Record` per dispatched event; only `data:` matters because the event JSON carries `seq`.
#[derive(Default)]
pub(crate) struct Decoder {
    line: Vec<u8>,
    data: Vec<u8>,
    after_cr: bool,
}

pub(crate) struct Record {
    pub(crate) data: Vec<u8>,
}

impl Decoder {
    pub(crate) fn push(&mut self, bytes: &[u8]) -> Vec<Record> {
        let mut records = Vec::new();
        for &byte in bytes {
            if self.after_cr {
                self.after_cr = false;
                if byte == b'\n' {
                    continue;
                }
            }
            if byte == b'\r' || byte == b'\n' {
                self.finish_line(&mut records);
                self.after_cr = byte == b'\r';
            } else {
                self.line.push(byte);
            }
        }
        records
    }

    fn finish_line(&mut self, records: &mut Vec<Record>) {
        let line = std::mem::take(&mut self.line);
        if line.is_empty() {
            if !self.data.is_empty() {
                self.data.pop();
                records.push(Record {
                    data: std::mem::take(&mut self.data),
                });
            }
            return;
        }
        if line.starts_with(b":") {
            return;
        }
        let (field, mut value) = match line.iter().position(|b| *b == b':') {
            Some(colon) => (&line[..colon], &line[colon + 1..]),
            None => (line.as_slice(), &[][..]),
        };
        if value.starts_with(b" ") {
            value = &value[1..];
        }
        if field == b"data" {
            self.data.extend_from_slice(value);
            self.data.push(b'\n');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_split_across_chunks_and_comments_are_ignored() {
        let mut decoder = Decoder::default();
        assert!(decoder
            .push(b": keep-alive\r\nid: 4\r\ndata: {\"a\"")
            .is_empty());
        let records = decoder.push(b":1}\r\n\r\nid: 5\ndata: x\ndata: y\n\n");
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].data, b"{\"a\":1}");
        assert_eq!(records[1].data, b"x\ny");
    }
}
