use mail_parser::{Address, HeaderValue, MessageParser};

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
#[cfg(test)]
mod tests {
    use super::*;

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
    fn bare_addresses_extracts_lowercase_addresses() {
        assert_eq!(
            bare_addresses("Bob <Bob@Example.com>, carol@example.com"),
            vec!["bob@example.com", "carol@example.com"]
        );
        assert!(bare_addresses("").is_empty());
    }
}
