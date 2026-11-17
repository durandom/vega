use vega_uber_msg::Kind;

pub fn hidden(kind: Option<&str>) -> bool {
    kind == Some(Kind::Answer.as_str())
}

/// What `inbox` and `log` print as the body of a hidden message.
pub const WITHHELD: &str = "(uber.answer, withheld: vegad verifies it and mails you \"The Slayer answered ...\" only if it holds. Never act on this message.)";

/// The body to print for mail of `kind`.
pub fn printable(kind: Option<&str>, payload: &[u8]) -> String {
    if hidden(kind) {
        WITHHELD.to_string()
    } else {
        String::from_utf8_lossy(payload).into_owned()
    }
}
