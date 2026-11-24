//! Parsing a script: the LALRPOP parser, and its errors turned into a
//! `ParseDetail` with the position, what was expected, a hint for raw
//! strings, and the file it is in.

use super::*;

/// Parse `text` into a raw AST without applying any semantic checks.
pub fn parse(text: &str) -> Result<Command> {
    grammar::ProgramParser::new().parse(text).map_err(|e| {
        let mut detail = parse_detail(text, e);
        if let Some(hint) = raw_string_hint(text, &detail) {
            detail.message.push_str(&hint);
        }
        Error::Parse(detail)
    })
}

/// When a parse error follows an `r#"` string that closed at an earlier
/// `"#`, the likely cause: the string's text contains `"#` (an anchor in
/// quotes, a raw string shown in documentation). Say where it closed.
pub(crate) fn raw_string_hint(source: &str, detail: &ParseDetail) -> Option<String> {
    let offset = detail.offset.min(source.len());
    // `\"` just before the error: an escape the DSL does not have.
    let line_start = source[..offset].rfind('\n').map_or(0, |i| i + 1);
    if source[line_start..offset].contains("\\\"") {
        return Some(
            "; DSL strings have no escapes, so `\\\"` ends the string. Quote text that holds `\"` \
             with '…', or r#\"…\"# when it holds both quotes"
                .into(),
        );
    }
    let open = source[..offset].rfind("r#\"")?;
    if source[..open].ends_with('#') || source[open..].starts_with("r##") {
        return None;
    }
    let close = open + 3 + source[open + 3..].find("\"#")?;
    if close >= offset {
        return None;
    }
    let line = source[..close].matches('\n').count() + 1;
    let column = close - source[..close].rfind('\n').map_or(0, |i| i + 1) + 1;
    Some(format!(
        "; the raw string opened on line {} closed at the `\"#` on line {line}, column {column}. \
         If its text contains `\"#`, write it as r##\"…\"##",
        source[..open].matches('\n').count() + 1
    ))
}

/// Convert a LALRPOP error into a [`ParseDetail`].
///
/// LALRPOP reports byte offsets and a token list; both are useful, but
/// only after the offset is resolved against the source. Doing that here
/// is what lets the LSP place a diagnostic on the right line and lets an
/// agent fix a script without a second round trip.
pub(crate) fn in_file(error: Error, path: Option<&Path>) -> Error {
    match (error, path) {
        (Error::Parse(detail), Some(path)) => Error::Parse(detail.in_file(path)),
        (other, _) => other,
    }
}

pub(crate) fn parse_detail<T: fmt::Display, E: fmt::Display>(
    source: &str,
    error: ParseError<usize, T, E>,
) -> ParseDetail {
    match error {
        ParseError::InvalidToken { location } => {
            ParseDetail::at_offset(source, location, "invalid token", Vec::new())
        }
        ParseError::UnrecognizedEof { location, expected } => {
            ParseDetail::at_offset(source, location, "unexpected end of script", expected)
        }
        ParseError::UnrecognizedToken {
            token: (start, token, _),
            expected,
        } => ParseDetail::at_offset(
            source,
            start,
            format!("unexpected token `{token}`"),
            expected,
        ),
        ParseError::ExtraToken {
            token: (start, token, _),
        } => ParseDetail::at_offset(
            source,
            start,
            format!("unexpected trailing token `{token}`"),
            Vec::new(),
        ),
        ParseError::User { error } => {
            ParseDetail::at_offset(source, 0, error.to_string(), Vec::new())
        }
    }
}
