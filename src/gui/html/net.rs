//! Resources for an HTML body: `cid:` parts of the message and `data:` URIs. Nothing else is fetched.
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use blitz_traits::net::{Bytes, NetHandler, NetProvider, Request};

use crate::message::InlinePart;

/// Answers every request at once, from memory. A refused request gets empty bytes, so a stylesheet in `<head>` does not
/// hold up painting and an image simply fails to decode.
pub(crate) struct Inline {
    parts: Vec<InlinePart>,
    refused: Arc<AtomicBool>,
}

impl Inline {
    /// The provider, and a flag it sets when it refused a request for remote content.
    pub fn new(parts: Vec<InlinePart>) -> (Inline, Arc<AtomicBool>) {
        let refused = Arc::new(AtomicBool::new(false));
        let provider = Inline {
            parts,
            refused: refused.clone(),
        };
        (provider, refused)
    }

    fn resolve(&self, request: &Request) -> Option<Vec<u8>> {
        let url = &request.url;
        match url.scheme() {
            "cid" => {
                let cid = percent_decode(url.path());
                self.parts
                    .iter()
                    .find(|part| part.cid == cid)
                    .map(|part| part.bytes.clone())
            }
            "data" => {
                let data = data_url::DataUrl::process(url.as_str()).ok()?;
                data.decode_to_vec().ok().map(|(bytes, _)| bytes)
            }
            "http" | "https" => {
                self.refused.store(true, Ordering::Relaxed);
                None
            }
            _ => None,
        }
    }
}

impl NetProvider for Inline {
    fn fetch(&self, _doc_id: usize, request: Request, handler: Box<dyn NetHandler>) {
        let bytes = self.resolve(&request).unwrap_or_default();
        handler.bytes(request.url.to_string(), Bytes::from(bytes));
    }
}

/// `%40` back to `@`; a `%` not followed by two hex digits stays as it is.
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|pair| std::str::from_utf8(pair).ok())
            .and_then(|pair| u8::from_str_radix(pair, 16).ok());
        match (bytes[i], hex) {
            (b'%', Some(byte)) => {
                out.push(byte);
                i += 3;
            }
            (byte, _) => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use blitz_traits::net::Url;

    use super::*;

    struct Capture(Arc<Mutex<Option<Bytes>>>);

    impl NetHandler for Capture {
        fn bytes(self: Box<Self>, _resolved_url: String, bytes: Bytes) {
            *self.0.lock().unwrap() = Some(bytes);
        }
    }

    fn fetch(provider: &Inline, url: &str) -> Vec<u8> {
        let got = Arc::new(Mutex::new(None));
        let request = Request::get(Url::parse(url).unwrap());
        provider.fetch(0, request, Box::new(Capture(got.clone())));
        let answered = got.lock().unwrap().take();
        answered
            .expect("every request is answered at once")
            .to_vec()
    }

    fn provider() -> (Inline, Arc<AtomicBool>) {
        Inline::new(vec![InlinePart {
            cid: "logo@example.com".into(),
            bytes: b"PNG".to_vec(),
        }])
    }

    #[test]
    fn cid_and_data_resolve_from_memory() {
        let (inline, refused) = provider();
        assert_eq!(fetch(&inline, "cid:logo@example.com"), b"PNG");
        assert_eq!(fetch(&inline, "cid:logo%40example.com"), b"PNG");
        assert_eq!(fetch(&inline, "data:text/plain;base64,aGk="), b"hi");
        assert_eq!(fetch(&inline, "data:,a%20b"), b"a b");
        assert!(!refused.load(Ordering::Relaxed));
    }

    #[test]
    fn everything_else_is_answered_empty_and_remote_is_noted() {
        let (inline, refused) = provider();
        assert!(fetch(&inline, "cid:unknown").is_empty());
        assert!(fetch(&inline, "file:///etc/passwd").is_empty());
        assert!(fetch(&inline, "postvak://mail/relative.png").is_empty());
        assert!(!refused.load(Ordering::Relaxed));
        assert!(fetch(&inline, "https://tracker.example.com/p.gif").is_empty());
        assert!(refused.load(Ordering::Relaxed));
    }

    #[test]
    fn percent_decode_keeps_stray_percent_signs() {
        assert_eq!(percent_decode("a%40b%2"), "a@b%2");
        assert_eq!(percent_decode("%zz"), "%zz");
    }
}
