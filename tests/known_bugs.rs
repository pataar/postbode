// Test code: unwrap, expect and panic are how a test fails.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Panics found by `tests/fuzz.rs` that are not fixed yet, one minimal reproduction each. Run them with
//! `cargo test --test known_bugs -- --ignored`; a fixed bug's test passes and moves to its module's tests.
//!
//! Both bugs below are in mail-parser 0.11.9 (the latest release) and only fire when mail-parser is built with debug
//! assertions, as `cargo test` and `cargo run` do. A release build skips the check and carries on, so the shipped
//! binary does not panic. `tests/fuzz.rs` tolerates exactly these two panic sites so the property tests stay stable.

use postvak::message::{attachments, body_text};

#[test]
#[ignore = "BUG: mail-parser 0.11.9 subtracts with overflow (decoders/quoted_printable.rs:138) on a quoted-printable part holding only a soft line break; debug builds only"]
fn quoted_printable_part_with_only_a_soft_line_break() {
    let raw = b"Content-Type: multipart/mixed; boundary=\"b\"\r\n\
\r\n\
--b\r\n\
Content-Type: text/plain\r\n\
Content-Transfer-Encoding: quoted-printable\r\n\
\r\n\
=\r\n\
--b--\r\n";
    body_text(raw);
    attachments(raw);
}

#[test]
#[ignore = "BUG: mail-parser 0.11.9 hits debug_assert!(false, \"Invalid part ID, could not find multipart.\") (parsers/message.rs:485) on nested message/rfc822 parts after a malformed boundary line; debug builds only"]
fn nested_rfc822_after_malformed_boundary() {
    let raw = b"Content-Type: multipart/mixed; boundary=outer\n\
\n\
--outerContent-Type: message/rfc822\n\
C\n\
\n\
Content-Type: message/rfc822\n\
\n\
\n\
--outer--";
    body_text(raw);
    attachments(raw);
}
