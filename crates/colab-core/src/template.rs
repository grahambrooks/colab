//! Tiny string template engine used by `replace_call` and any
//! future template-based actions.
//!
//! Placeholders:
//!
//! - `$1`, `$2`, … — 1-indexed positional argument. Out-of-range
//!   indices expand to the empty string.
//! - `$args` — the argument list, joined with `", "`.
//! - `$func` — the matched function name.
//! - `$$` — a literal `$`.
//!
//! Anything else after a `$` (including unknown identifiers) is
//! emitted verbatim — the parser is intentionally forgiving so a
//! template like `Some($1)` works without escaping the `$`.

/// Render `template` with the given function name and positional
/// arguments. `args` slices should be the verbatim source text of
/// each argument (whitespace-trimmed by the caller if desired).
pub fn render_call_template(template: &str, function_name: &str, args: &[&str]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;

    while let Some(idx) = rest.find('$') {
        out.push_str(&rest[..idx]);
        rest = &rest[idx + 1..];

        if let Some(stripped) = rest.strip_prefix('$') {
            out.push('$');
            rest = stripped;
        } else if let Some(stripped) = rest.strip_prefix("args") {
            out.push_str(&args.join(", "));
            rest = stripped;
        } else if let Some(stripped) = rest.strip_prefix("func") {
            out.push_str(function_name);
            rest = stripped;
        } else {
            // 1+ ASCII digits → positional placeholder.
            let digit_end = rest
                .as_bytes()
                .iter()
                .take_while(|b| b.is_ascii_digit())
                .count();
            if digit_end > 0 {
                let idx_str = &rest[..digit_end];
                let idx: usize = idx_str.parse().unwrap_or(0);
                if idx >= 1
                    && idx <= args.len()
                {
                    out.push_str(args[idx - 1]);
                }
                rest = &rest[digit_end..];
            } else {
                // Unknown placeholder: emit the literal `$` and
                // leave the rest for the next loop.
                out.push('$');
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn substitutes_args_placeholder() {
        let out = render_call_template("f($args)", "old", &["a", "b"]);
        assert_eq!(out, "f(a, b)");
    }

    #[test]
    fn substitutes_positional_args() {
        let out = render_call_template("g($2, $1, nil)", "old", &["a", "b"]);
        assert_eq!(out, "g(b, a, nil)");
    }

    #[test]
    fn substitutes_func_placeholder() {
        let out = render_call_template("$func($args)", "pkg.Old", &["x"]);
        assert_eq!(out, "pkg.Old(x)");
    }

    #[test]
    fn dollar_dollar_emits_literal() {
        assert_eq!(render_call_template("$$1", "f", &["a"]), "$1");
    }

    #[test]
    fn out_of_range_positional_expands_to_empty() {
        let out = render_call_template("h($1, $5)", "old", &["a"]);
        assert_eq!(out, "h(a, )");
    }

    #[test]
    fn unknown_placeholder_emits_literal_dollar() {
        let out = render_call_template("$bogus", "old", &[]);
        assert_eq!(out, "$bogus");
    }

    #[test]
    fn supports_no_args_call() {
        let out = render_call_template("g()", "old", &[]);
        assert_eq!(out, "g()");
    }

    #[test]
    fn handles_template_without_placeholders() {
        assert_eq!(render_call_template("plain text", "f", &[]), "plain text");
    }
}
