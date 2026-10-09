use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use mail_parser::{Address, HeaderValue, MessageParser, MimeHeaders};

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Parsed {
    pub message_id: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub cc: Option<String>,
    pub delivered_to: Option<String>,
    pub subject: Option<String>,
    pub date: Option<i64>,
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
}

pub fn parse_headers(raw: &[u8]) -> Parsed {
    let Some(msg) = MessageParser::default().parse_headers(raw) else {
        return Parsed::default();
    };
    Parsed {
        message_id: msg.message_id().map(str::to_string),
        from: msg.from().map(format_address),
        to: msg.to().map(format_address),
        cc: msg.cc().map(format_address),
        delivered_to: msg.header_raw("Delivered-To").map(|v| v.trim().to_string()),
        subject: msg.subject().map(str::to_string),
        date: msg.date().map(|d| d.to_timestamp()),
        in_reply_to: text_list(msg.in_reply_to()).into_iter().next(),
        references: text_list(msg.references()),
    }
}

pub fn header_value(raw_headers: &[u8], name: &str) -> Option<String> {
    let msg = MessageParser::default().parse_headers(raw_headers)?;
    msg.header_raw(name).map(|v| v.trim().to_string())
}

pub fn synthetic_message_id(folder: &str, uid: u32) -> String {
    format!("{uid}@{folder}.postbode")
}

pub fn thread_id(parsed: &Parsed, folder: &str, uid: u32) -> String {
    parsed
        .references
        .first()
        .cloned()
        .or_else(|| parsed.in_reply_to.clone())
        .or_else(|| parsed.message_id.clone())
        .unwrap_or_else(|| synthetic_message_id(folder, uid))
}

pub fn body_text(raw: &[u8]) -> String {
    let Some(msg) = MessageParser::default().parse(raw) else {
        return String::new();
    };
    msg.body_text(0)
        .map(|text| text.into_owned())
        .unwrap_or_default()
}

/// The HTML of a message and the parts its `cid:` URLs can name.
#[derive(Debug, Clone, PartialEq)]
pub struct HtmlBody {
    pub html: String,
    pub inline: Vec<InlinePart>,
}

/// A part with a `Content-ID`, which the HTML shows as `<img src="cid:…">`.
#[derive(Debug, Clone, PartialEq)]
pub struct InlinePart {
    /// The Content-ID without its angle brackets.
    pub cid: String,
    pub bytes: Vec<u8>,
}

/// The first `text/html` part with every `Content-ID` part, or `None` when the message has no HTML part.
pub fn html_body(raw: &[u8]) -> Option<HtmlBody> {
    let msg = MessageParser::default().parse(raw)?;
    let part = msg.html_part(0).filter(|part| part.is_text_html())?;
    let html = part.text_contents()?.to_string();
    let inline = msg
        .parts
        .iter()
        .filter_map(|part| {
            let cid = part
                .content_id()?
                .trim()
                .trim_start_matches('<')
                .trim_end_matches('>');
            (!cid.is_empty()).then(|| InlinePart {
                cid: cid.to_string(),
                bytes: part.contents().to_vec(),
            })
        })
        .collect();
    Some(HtmlBody { html, inline })
}

pub fn bare_addresses(field: &str) -> Vec<String> {
    field
        .split(',')
        .filter_map(|part| {
            let part = part.trim();
            if part.is_empty() {
                return None;
            }
            let addr = match (part.rfind('<'), part.rfind('>')) {
                (Some(start), Some(end)) if end > start => &part[start + 1..end],
                _ => part,
            };
            Some(addr.trim().to_ascii_lowercase())
        })
        .collect()
}

fn format_address(address: &Address<'_>) -> String {
    address
        .iter()
        .map(|a| match (a.name(), a.address()) {
            (Some(name), Some(addr)) => format!("{name} <{addr}>"),
            (None, Some(addr)) => addr.to_string(),
            (Some(name), None) => name.to_string(),
            (None, None) => String::new(),
        })
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(", ")
}

fn text_list(value: &HeaderValue<'_>) -> Vec<String> {
    match value {
        HeaderValue::Text(t) => vec![t.to_string()],
        HeaderValue::TextList(list) => list.iter().map(|t| t.to_string()).collect(),
        _ => vec![],
    }
}
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Attachment {
    /// 1-based, as `attachment save` takes it.
    pub index: usize,
    pub name: Option<String>,
    pub content_type: String,
    pub size: usize,
}

pub fn attachments(raw: &[u8]) -> Vec<Attachment> {
    let Some(msg) = MessageParser::default().parse(raw) else {
        return Vec::new();
    };
    msg.attachments()
        .enumerate()
        .map(|(i, part)| Attachment {
            index: i + 1,
            name: part.attachment_name().map(str::to_string),
            content_type: part
                .content_type()
                .map(|ct| match ct.subtype() {
                    Some(sub) => format!("{}/{sub}", ct.ctype()),
                    None => ct.ctype().to_string(),
                })
                .unwrap_or_else(|| "application/octet-stream".into()),
            size: part.contents().len(),
        })
        .collect()
}

/// Writes attachment `index` (1-based) into `dir` under its own file name, stripped of any path; never overwrites.
pub fn save_attachment(raw: &[u8], index: usize, dir: &Path) -> io::Result<PathBuf> {
    let msg = MessageParser::default()
        .parse(raw)
        .ok_or_else(|| io::Error::other("the message could not be parsed"))?;
    let part = index
        .checked_sub(1)
        .and_then(|i| msg.attachment(u32::try_from(i).ok()?))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("no attachment {index}")))?;
    let path = dir.join(safe_file_name(part.attachment_name(), index));
    write_new(&path, part.contents())?;
    Ok(path)
}

/// The sender picks the name: keep only its last path segment, without control characters or leading dots, so it
/// cannot be a hidden file such as `.bash_profile`.
fn safe_file_name(name: Option<&str>, index: usize) -> String {
    let base: String = name
        .unwrap_or("")
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("")
        .chars()
        .filter(|c| !c.is_control())
        .collect();
    match base
        .trim_start_matches(|c: char| c == '.' || c.is_whitespace())
        .trim_end()
    {
        "" => format!("attachment-{index}"),
        name => name.to_string(),
    }
}

/// Bytes kept of a subject in a file name; well under the 255-byte limit of APFS and ext4, with room for " (100).eml".
const MAX_STEM: usize = 120;

/// Numbered names tried after the plain one before Save .eml gives up.
const MAX_COPIES: u32 = 100;

/// Writes `raw` unchanged to `dir` as `<subject>.eml`, or `<subject> (2).eml` and on when that name is taken; never
/// overwrites a file.
pub fn save_eml(raw: &[u8], subject: Option<&str>, dir: &Path) -> io::Result<PathBuf> {
    let stem = eml_stem(subject);
    for copy in 1..=MAX_COPIES {
        let name = match copy {
            1 => format!("{stem}.eml"),
            n => format!("{stem} ({n}).eml"),
        };
        let path = dir.join(name);
        match write_new(&path, raw) {
            Ok(()) => return Ok(path),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        format!("{stem}.eml and {MAX_COPIES} numbered copies exist"),
    ))
}

/// The sender picks the subject: no path separators or `:` (Finder shows it as `/`), no control characters, no leading
/// dots, so it is one visible file name.
fn eml_stem(subject: Option<&str>) -> String {
    let plain: String = subject
        .unwrap_or("")
        .chars()
        .filter(|c| !c.is_control())
        .map(|c| {
            if matches!(c, '/' | '\\' | ':') {
                '-'
            } else {
                c
            }
        })
        .collect();
    let trimmed = plain.trim_start_matches(|c: char| c == '.' || c.is_whitespace());
    match trimmed[..trimmed.floor_char_boundary(MAX_STEM)].trim_end() {
        "" => "message".into(),
        stem => stem.into(),
    }
}

/// Creates `path`, failing when it exists, and writes `bytes`; a failed write leaves no partial file.
fn write_new(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    if let Err(e) = file.write_all(bytes) {
        let _ = std::fs::remove_file(path);
        return Err(e);
    }
    Ok(())
}

/// Server-supplied text with control characters removed, so a header cannot drive the terminal. `keep_layout` keeps
/// newlines and tabs, for message bodies.
pub fn clean(text: &str, keep_layout: bool) -> String {
    text.chars()
        .filter(|c| !c.is_control() || (keep_layout && matches!(c, '\n' | '\t')))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_strips_control_characters() {
        let hostile = "Re: \u{1b}]0;pwned\u{7}hi\u{9b}2J\r\n\tthere\u{7f}";
        assert_eq!(clean(hostile, false), "Re: ]0;pwnedhi2Jthere");
        assert_eq!(clean(hostile, true), "Re: ]0;pwnedhi2J\n\tthere");
    }

    const HEADERS: &[u8] = b"From: Alice <alice@example.com>\r\n\
To: Bob <bob@example.com>, carol@example.com\r\n\
Cc: dave@example.com\r\n\
Delivered-To: bob@example.com\r\n\
Subject: Re: lunch\r\n\
Date: Mon, 6 Oct 2026 12:00:00 +0200\r\n\
Message-ID: <m3@example.com>\r\n\
In-Reply-To: <m2@example.com>\r\n\
References: <m1@example.com> <m2@example.com>\r\n\
List-Id: Dev <dev.lists.example.com>\r\n\
\r\n";

    #[test]
    fn parses_common_headers() {
        let p = parse_headers(HEADERS);
        assert_eq!(p.from.as_deref(), Some("Alice <alice@example.com>"));
        assert_eq!(
            p.to.as_deref(),
            Some("Bob <bob@example.com>, carol@example.com")
        );
        assert_eq!(p.cc.as_deref(), Some("dave@example.com"));
        assert_eq!(p.delivered_to.as_deref(), Some("bob@example.com"));
        assert_eq!(p.subject.as_deref(), Some("Re: lunch"));
        assert_eq!(p.message_id.as_deref(), Some("m3@example.com"));
        assert_eq!(p.in_reply_to.as_deref(), Some("m2@example.com"));
        assert_eq!(p.references, vec!["m1@example.com", "m2@example.com"]);
        assert_eq!(p.date, Some(1_791_280_800));
    }

    #[test]
    fn header_value_is_case_insensitive() {
        assert_eq!(
            header_value(HEADERS, "list-id").as_deref(),
            Some("Dev <dev.lists.example.com>")
        );
        assert_eq!(header_value(HEADERS, "X-Missing"), None);
    }

    #[test]
    fn thread_id_prefers_first_reference_then_in_reply_to_then_self() {
        let p = parse_headers(HEADERS);
        assert_eq!(thread_id(&p, "INBOX", 1), "m1@example.com");
        let no_refs = Parsed {
            references: vec![],
            ..p.clone()
        };
        assert_eq!(thread_id(&no_refs, "INBOX", 1), "m2@example.com");
        let root = Parsed {
            references: vec![],
            in_reply_to: None,
            ..p.clone()
        };
        assert_eq!(thread_id(&root, "INBOX", 1), "m3@example.com");
    }

    #[test]
    fn thread_id_without_message_id_is_synthetic_and_stable() {
        let p = Parsed::default();
        assert_eq!(thread_id(&p, "INBOX", 42), "42@INBOX.postbode");
        assert_eq!(
            thread_id(&p, "INBOX", 42),
            synthetic_message_id("INBOX", 42)
        );
    }

    #[test]
    fn body_text_prefers_plain_and_falls_back_to_html() {
        let plain = b"From: a@b\r\nContent-Type: text/plain\r\n\r\nhello plain\r\n";
        assert_eq!(body_text(plain).trim(), "hello plain");
        let html = b"From: a@b\r\nContent-Type: text/html\r\n\r\n<p>hello <b>html</b></p>\r\n";
        assert!(body_text(html).contains("hello html"));
    }

    #[test]
    fn html_body_is_the_html_part_or_none() {
        let html = b"From: a@b\r\nContent-Type: text/html\r\n\r\n<p>hello</p>\r\n";
        let body = html_body(html).unwrap();
        assert_eq!(body.html.trim(), "<p>hello</p>");
        assert!(body.inline.is_empty());
        let plain = b"From: a@b\r\nContent-Type: text/plain\r\n\r\n<p>not html</p>\r\n";
        assert_eq!(html_body(plain), None);
        assert_eq!(html_body(b""), None);
    }

    #[test]
    fn html_body_prefers_html_in_an_alternative() {
        let raw = b"From: a@b\r\nMIME-Version: 1.0\r\nContent-Type: multipart/alternative; boundary=\"b\"\r\n\r\n\
--b\r\nContent-Type: text/plain\r\n\r\nplain\r\n\
--b\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<b>rich</b>\r\n--b--\r\n";
        assert_eq!(html_body(raw).unwrap().html.trim(), "<b>rich</b>");
    }

    #[test]
    fn html_body_carries_the_content_id_parts() {
        let raw = b"From: a@b\r\nMIME-Version: 1.0\r\nContent-Type: multipart/related; boundary=\"b\"\r\n\r\n\
--b\r\nContent-Type: text/html\r\n\r\n<img src=\"cid:logo@x\"><img src=\"cid:sig\">\r\n\
--b\r\nContent-Type: image/png\r\nContent-ID: <logo@x>\r\nContent-Transfer-Encoding: base64\r\n\r\niVBORw==\r\n\
--b\r\nContent-Type: image/gif\r\nContent-ID: sig\r\n\r\nGIF89a\r\n--b--\r\n";
        let body = html_body(raw).unwrap();
        let cids: Vec<_> = body
            .inline
            .iter()
            .map(|p| (p.cid.as_str(), p.bytes.clone()))
            .collect();
        assert_eq!(
            cids,
            [("logo@x", b"\x89PNG".to_vec()), ("sig", b"GIF89a".to_vec())]
        );
    }

    #[test]
    fn bare_addresses_extracts_lowercase_addresses() {
        assert_eq!(
            bare_addresses("Bob <Bob@Example.com>, carol@example.com"),
            vec!["bob@example.com", "carol@example.com"]
        );
        assert!(bare_addresses("").is_empty());
    }

    const WITH_ATTACHMENTS: &[u8] = b"From: a@example.com\r\n\
Subject: files\r\n\
MIME-Version: 1.0\r\n\
Content-Type: multipart/mixed; boundary=\"b\"\r\n\
\r\n\
--b\r\n\
Content-Type: text/plain\r\n\
\r\n\
see attached\r\n\
--b\r\n\
Content-Type: text/plain\r\n\
Content-Disposition: attachment; filename=\"../../evil.txt\"\r\n\
\r\n\
not evil\r\n\
--b\r\n\
Content-Type: application/pdf\r\n\
Content-Disposition: attachment\r\n\
Content-Transfer-Encoding: base64\r\n\
\r\n\
JVBERi0=\r\n\
--b--\r\n";

    #[test]
    fn attachments_are_listed_and_saved_inside_the_directory() {
        let found = attachments(WITH_ATTACHMENTS);
        let summary: Vec<_> = found
            .iter()
            .map(|a| (a.index, a.name.as_deref(), a.content_type.as_str()))
            .collect();
        assert_eq!(
            summary,
            [
                (1, Some("../../evil.txt"), "text/plain"),
                (2, None, "application/pdf")
            ]
        );
        assert_eq!(found[1].size, 5);
        let dir = tempfile::tempdir().unwrap();
        let saved = save_attachment(WITH_ATTACHMENTS, 1, dir.path()).unwrap();
        assert_eq!(saved, dir.path().join("evil.txt"));
        assert_eq!(
            std::fs::read_to_string(&saved).unwrap().trim_end(),
            "not evil"
        );
        let again = save_attachment(WITH_ATTACHMENTS, 1, dir.path()).unwrap_err();
        assert_eq!(again.kind(), std::io::ErrorKind::AlreadyExists);
        let pdf = save_attachment(WITH_ATTACHMENTS, 2, dir.path()).unwrap();
        assert_eq!(pdf, dir.path().join("attachment-2"));
        assert_eq!(std::fs::read(pdf).unwrap(), b"%PDF-");
        for missing in [0, 3] {
            let err = save_attachment(WITH_ATTACHMENTS, missing, dir.path()).unwrap_err();
            assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
        }
    }

    #[test]
    fn unsafe_attachment_names_become_plain_file_names() {
        assert_eq!(safe_file_name(Some("../../.ssh/config"), 1), "config");
        assert_eq!(
            safe_file_name(Some("C:\\Users\\x\\evil.exe"), 1),
            "evil.exe"
        );
        assert_eq!(safe_file_name(Some(".."), 3), "attachment-3");
        assert_eq!(safe_file_name(Some("a\u{1b}b.txt"), 1), "ab.txt");
        assert_eq!(safe_file_name(None, 2), "attachment-2");
    }

    #[test]
    fn safe_file_name_strips_leading_dots() {
        assert_eq!(safe_file_name(Some(".bash_profile"), 1), "bash_profile");
        assert_eq!(safe_file_name(Some("../.ssh/.config"), 1), "config");
        assert_eq!(safe_file_name(Some(" ..."), 4), "attachment-4");
        assert_eq!(safe_file_name(Some("report.v2.pdf"), 1), "report.v2.pdf");
    }

    #[test]
    fn eml_names_come_from_the_subject_and_stay_plain_file_names() {
        assert_eq!(eml_stem(Some("Re: lunch")), "Re- lunch");
        assert_eq!(
            eml_stem(Some("../../.ssh/authorized_keys")),
            "-..-.ssh-authorized_keys"
        );
        assert_eq!(eml_stem(Some("\u{1b}]0;pwned\u{7}hi")), "]0;pwnedhi");
        assert_eq!(eml_stem(Some(" ... ")), "message");
        assert_eq!(eml_stem(None), "message");
        assert_eq!(eml_stem(Some(&"é".repeat(100))), "é".repeat(60));
    }

    #[test]
    fn save_eml_never_overwrites_and_numbers_the_copy() {
        let dir = tempfile::tempdir().unwrap();
        let first = save_eml(b"one", Some("Re: lunch"), dir.path()).unwrap();
        let second = save_eml(b"two", Some("Re: lunch"), dir.path()).unwrap();
        assert_eq!(first, dir.path().join("Re- lunch.eml"));
        assert_eq!(second, dir.path().join("Re- lunch (2).eml"));
        assert_eq!(std::fs::read(&first).unwrap(), b"one");
        assert_eq!(std::fs::read(&second).unwrap(), b"two");
    }

    #[test]
    fn save_eml_reports_a_missing_directory() {
        let dir = tempfile::tempdir().unwrap();
        let err = save_eml(b"x", None, &dir.path().join("gone")).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }
}
