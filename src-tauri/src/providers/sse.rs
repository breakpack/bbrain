/// Server-sent events arrive as arbitrary byte chunks, and a chunk boundary can
/// fall inside a multi-byte UTF-8 character. Decoding each chunk on its own
/// (`from_utf8_lossy`) corrupts those characters, which silently breaks every
/// Korean delta while leaving the ASCII-only control events intact.
///
/// So buffer bytes, split on newlines, and only then decode — a complete line is
/// always valid UTF-8.
#[derive(Default)]
pub struct SseDecoder {
    buffer: Vec<u8>,
}

impl SseDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds a chunk and returns the `data:` payloads that are now complete.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        self.buffer.extend_from_slice(chunk);
        let mut payloads = Vec::new();

        while let Some(index) = self.buffer.iter().position(|byte| *byte == b'\n') {
            let line: Vec<u8> = self.buffer.drain(..=index).collect();

            let Ok(line) = std::str::from_utf8(&line) else {
                // A line that is not valid UTF-8 cannot be a JSON event.
                continue;
            };

            if let Some(data) = line.trim().strip_prefix("data:") {
                let data = data.trim();
                if !data.is_empty() && data != "[DONE]" {
                    payloads.push(data.to_string());
                }
            }
        }

        payloads
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_whole_event_from_one_chunk() {
        let mut decoder = SseDecoder::new();

        let events = decoder.push(b"data: {\"a\":1}\n\n");

        assert_eq!(events, vec!["{\"a\":1}"]);
    }

    #[test]
    fn a_korean_delta_split_across_chunks_survives() {
        let mut decoder = SseDecoder::new();
        let payload = "data: {\"text\":\"안녕하세요\"}\n";
        let bytes = payload.as_bytes();

        // Split inside the first Hangul syllable: exactly what reqwest does, and
        // what a per-chunk lossy decode would corrupt.
        let split = payload.find('안').unwrap() + 1;
        assert!(decoder.push(&bytes[..split]).is_empty());
        let events = decoder.push(&bytes[split..]);

        assert_eq!(events.len(), 1);
        let value: serde_json::Value = serde_json::from_str(&events[0]).unwrap();
        assert_eq!(value["text"], "안녕하세요");
    }

    #[test]
    fn several_events_in_one_chunk_all_come_out() {
        let mut decoder = SseDecoder::new();

        let events = decoder.push(b"data: {\"a\":1}\ndata: {\"b\":2}\n");

        assert_eq!(events, vec!["{\"a\":1}", "{\"b\":2}"]);
    }

    #[test]
    fn event_and_comment_lines_are_ignored() {
        let mut decoder = SseDecoder::new();

        let events = decoder.push(b"event: content_block_delta\n: ping\ndata: {\"a\":1}\n");

        assert_eq!(events, vec!["{\"a\":1}"]);
    }

    #[test]
    fn the_done_sentinel_is_not_an_event() {
        let mut decoder = SseDecoder::new();

        assert!(decoder.push(b"data: [DONE]\n").is_empty());
    }

    #[test]
    fn an_incomplete_line_is_held_until_its_newline_arrives() {
        let mut decoder = SseDecoder::new();

        assert!(decoder.push(b"data: {\"a\":").is_empty());
        assert_eq!(decoder.push(b"1}\n"), vec!["{\"a\":1}"]);
    }
}
