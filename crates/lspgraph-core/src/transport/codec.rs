//! Content-Length framing for JSON-RPC over stdio.

/// Frame a payload with an LSP `Content-Length` header.
pub fn encode(payload: &[u8]) -> Vec<u8> {
    let mut out = format!("Content-Length: {}\r\n\r\n", payload.len()).into_bytes();
    out.extend_from_slice(payload);
    out
}

/// Incremental decoder. Bytes arrive in arbitrary chunks; messages come out whole.
#[derive(Default)]
pub struct Decoder {
    buf: Vec<u8>,
}

impl Decoder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    pub fn next_message(&mut self) -> Option<Vec<u8>> {
        let header_end = find_subslice(&self.buf, b"\r\n\r\n")?;
        let header = std::str::from_utf8(&self.buf[..header_end]).ok()?;

        let mut len: Option<usize> = None;
        for line in header.split("\r\n") {
            let (k, v) = line.split_once(':')?;
            if k.trim().eq_ignore_ascii_case("content-length") {
                len = v.trim().parse().ok();
            }
        }
        let len = len?;

        let body_start = header_end + 4;
        if self.buf.len() < body_start + len {
            return None;
        }
        let body = self.buf[body_start..body_start + len].to_vec();
        self.buf.drain(..body_start + len);
        Some(body)
    }
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_with_content_length() {
        assert_eq!(encode(b"{}"), b"Content-Length: 2\r\n\r\n{}".to_vec());
    }

    #[test]
    fn decodes_a_single_message() {
        let mut d = Decoder::new();
        d.push(&encode(b"{\"a\":1}"));
        assert_eq!(d.next_message().unwrap(), b"{\"a\":1}".to_vec());
        assert!(d.next_message().is_none());
    }

    #[test]
    fn decodes_two_messages_from_one_chunk() {
        let mut d = Decoder::new();
        let mut chunk = encode(b"{\"a\":1}");
        chunk.extend_from_slice(&encode(b"{\"b\":2}"));
        d.push(&chunk);
        assert_eq!(d.next_message().unwrap(), b"{\"a\":1}".to_vec());
        assert_eq!(d.next_message().unwrap(), b"{\"b\":2}".to_vec());
        assert!(d.next_message().is_none());
    }

    #[test]
    fn waits_for_a_message_split_mid_header() {
        let mut d = Decoder::new();
        let full = encode(b"{\"a\":1}");
        d.push(&full[..8]);
        assert!(d.next_message().is_none());
        d.push(&full[8..]);
        assert_eq!(d.next_message().unwrap(), b"{\"a\":1}".to_vec());
    }

    #[test]
    fn waits_for_a_message_split_mid_body() {
        let mut d = Decoder::new();
        let full = encode(b"{\"a\":1}");
        let split = full.len() - 3;
        d.push(&full[..split]);
        assert!(d.next_message().is_none());
        d.push(&full[split..]);
        assert_eq!(d.next_message().unwrap(), b"{\"a\":1}".to_vec());
    }

    #[test]
    fn ignores_unknown_headers_and_is_case_insensitive() {
        let mut d = Decoder::new();
        d.push(b"content-type: application/json\r\nCONTENT-LENGTH: 2\r\n\r\n{}");
        assert_eq!(d.next_message().unwrap(), b"{}".to_vec());
    }
}
