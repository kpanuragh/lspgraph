//! Content-Length framing for JSON-RPC over stdio.

use crate::error::{Error, Result};

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

    pub fn next_message(&mut self) -> Result<Option<Vec<u8>>> {
        let header_end = match find_subslice(&self.buf, b"\r\n\r\n") {
            Some(pos) => pos,
            None => return Ok(None),
        };

        let header = match std::str::from_utf8(&self.buf[..header_end]) {
            Ok(h) => h.to_string(),
            Err(e) => {
                self.buf.drain(..header_end + 4);
                return Err(Error::Protocol(format!("invalid UTF-8 in header: {}", e)));
            }
        };

        let mut len: Option<usize> = None;
        let mut error_msg: Option<String> = None;

        for line in header.split("\r\n") {
            if line.is_empty() {
                continue;
            }
            match line.split_once(':') {
                Some((k, v)) => {
                    if k.trim().eq_ignore_ascii_case("content-length") {
                        len = v.trim().parse().ok();
                    }
                }
                None => {
                    error_msg = Some(format!("header line without colon: {}", line));
                    break;
                }
            }
        }

        if let Some(msg) = error_msg {
            self.buf.drain(..header_end + 4);
            return Err(Error::Protocol(msg));
        }

        let len = match len {
            Some(l) => l,
            None => {
                self.buf.drain(..header_end + 4);
                return Err(Error::Protocol(
                    "no Content-Length header found".to_string(),
                ));
            }
        };

        let body_start = header_end + 4;
        if self.buf.len() < body_start + len {
            return Ok(None);
        }
        let body = self.buf[body_start..body_start + len].to_vec();
        self.buf.drain(..body_start + len);
        Ok(Some(body))
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
        assert_eq!(d.next_message().unwrap().unwrap(), b"{\"a\":1}".to_vec());
        assert_eq!(d.next_message().unwrap(), None);
    }

    #[test]
    fn decodes_two_messages_from_one_chunk() {
        let mut d = Decoder::new();
        let mut chunk = encode(b"{\"a\":1}");
        chunk.extend_from_slice(&encode(b"{\"b\":2}"));
        d.push(&chunk);
        assert_eq!(d.next_message().unwrap().unwrap(), b"{\"a\":1}".to_vec());
        assert_eq!(d.next_message().unwrap().unwrap(), b"{\"b\":2}".to_vec());
        assert_eq!(d.next_message().unwrap(), None);
    }

    #[test]
    fn waits_for_a_message_split_mid_header() {
        let mut d = Decoder::new();
        let full = encode(b"{\"a\":1}");
        d.push(&full[..8]);
        assert_eq!(d.next_message().unwrap(), None);
        d.push(&full[8..]);
        assert_eq!(d.next_message().unwrap().unwrap(), b"{\"a\":1}".to_vec());
    }

    #[test]
    fn waits_for_a_message_split_mid_body() {
        let mut d = Decoder::new();
        let full = encode(b"{\"a\":1}");
        let split = full.len() - 3;
        d.push(&full[..split]);
        assert_eq!(d.next_message().unwrap(), None);
        d.push(&full[split..]);
        assert_eq!(d.next_message().unwrap().unwrap(), b"{\"a\":1}".to_vec());
    }

    #[test]
    fn ignores_unknown_headers_and_is_case_insensitive() {
        let mut d = Decoder::new();
        d.push(b"content-type: application/json\r\nCONTENT-LENGTH: 2\r\n\r\n{}");
        assert_eq!(d.next_message().unwrap().unwrap(), b"{}".to_vec());
    }

    #[test]
    fn header_line_without_colon_is_error_and_later_message_recovers() {
        let mut d = Decoder::new();
        // Push a message with invalid header (no colon), with no body after \r\n\r\n
        d.push(b"invalid line\r\n\r\n");
        let result = d.next_message();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("header line without colon"));

        // After error, buffer is drained and a new message can be decoded
        d.push(&encode(b"{\"a\":1}"));
        assert_eq!(d.next_message().unwrap().unwrap(), b"{\"a\":1}".to_vec());
    }

    #[test]
    fn missing_content_length_header_is_error() {
        let mut d = Decoder::new();
        d.push(b"content-type: application/json\r\n\r\n{}");
        let result = d.next_message();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("no Content-Length header found"));
    }

    #[test]
    fn invalid_utf8_in_header_is_error() {
        let mut d = Decoder::new();
        let mut invalid = vec![0xFF, 0xFE];
        invalid.extend_from_slice(b"\r\n\r\n{}");
        d.push(&invalid);
        let result = d.next_message();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("invalid UTF-8 in header"));
    }
}
