//! Desktop notifications for new mail, sent by the daemon.
use crate::message::clean;

pub fn new_mail(from: &str, subject: &str) {
    let _ = notify_rust::Notification::new()
        .summary(&notification_text(from))
        .body(&notification_text(subject))
        .appname("Postbode")
        .show();
}

/// Linux notification servers render a subset of HTML in the summary and body.
fn escape_markup(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn notification_text(text: &str) -> String {
    let text = clean(text, false);
    if cfg!(target_os = "linux") {
        escape_markup(&text)
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_markup_escapes_tags_and_entities() {
        assert_eq!(
            escape_markup("<b>Tom & Jerry</b>"),
            "&lt;b&gt;Tom &amp; Jerry&lt;/b&gt;"
        );
    }
}
