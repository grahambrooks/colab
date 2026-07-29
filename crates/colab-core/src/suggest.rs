//! "Did you mean …?" for the names a script can get wrong.
//!
//! Namespaces, modules, and language names are typed by hand (or by a
//! model working from memory), and a rejected name with no candidate list
//! costs a whole round trip to fix. Every site that rejects a name should
//! answer two questions at once: what was valid here, and which valid
//! name did you probably mean.

/// Levenshtein edit distance between two strings.
///
/// Two rolling rows rather than a full matrix — the inputs here are
/// identifiers, but there is no reason to allocate `n × m` for them.
fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }

    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr = vec![0usize; b.len() + 1];

    for (i, &ca) in a.iter().enumerate() {
        curr[0] = i + 1;
        for (j, &cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            curr[j + 1] = (prev[j] + cost).min(prev[j + 1] + 1).min(curr[j] + 1);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b.len()]
}

/// The candidate closest to `input`, or `None` if none is close enough
/// to be worth suggesting.
///
/// The threshold scales with the input length — one edit for very short
/// names, up to a third of the length for longer ones — so `improt` finds
/// `import` while `zzzzzz` suggests nothing.
pub fn closest<'a, I>(input: &str, candidates: I) -> Option<&'a str>
where
    I: IntoIterator<Item = &'a str>,
{
    let threshold = (input.chars().count() / 3).max(1);
    candidates
        .into_iter()
        .map(|candidate| (edit_distance(input, candidate), candidate))
        .filter(|(distance, _)| *distance <= threshold)
        .min_by_key(|(distance, candidate)| (*distance, candidate.len()))
        .map(|(_, candidate)| candidate)
}

/// Render the tail of an error message: the valid names, plus a
/// "did you mean" when one is close.
///
/// ```text
/// known modules: call, import, struct_tag, symbol (did you mean `import`?)
/// ```
pub fn candidates_note(input: &str, label: &str, candidates: &[&str]) -> String {
    if candidates.is_empty() {
        return format!("no {} are registered", label);
    }
    let mut sorted = candidates.to_vec();
    sorted.sort_unstable();
    let mut note = format!("known {}: {}", label, sorted.join(", "));
    if let Some(best) = closest(input, sorted.iter().copied()) {
        note.push_str(&format!(" (did you mean `{}`?)", best));
    }
    note
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distance_counts_single_edits() {
        assert_eq!(edit_distance("import", "import"), 0);
        assert_eq!(edit_distance("improt", "import"), 2);
        assert_eq!(edit_distance("", "abc"), 3);
        assert_eq!(edit_distance("abc", ""), 3);
    }

    #[test]
    fn finds_a_near_miss() {
        let modules = ["call", "import", "struct_tag", "symbol"];
        assert_eq!(closest("improt", modules), Some("import"));
        assert_eq!(closest("symbl", modules), Some("symbol"));
        assert_eq!(closest("Import", modules), Some("import"));
    }

    #[test]
    fn declines_to_guess_when_nothing_is_close() {
        let modules = ["call", "import", "struct_tag", "symbol"];
        assert_eq!(closest("zzzzzzzz", modules), None);
        assert_eq!(closest("", modules), None);
    }

    #[test]
    fn short_names_get_a_one_edit_budget() {
        // `rs` → `rust` is two edits; too far for a two-character input.
        assert_eq!(closest("rs", ["rust", "go"]), None);
        assert_eq!(closest("g0", ["rust", "go"]), Some("go"));
    }

    #[test]
    fn ties_prefer_the_shorter_candidate() {
        assert_eq!(closest("us", ["use", "uses"]), Some("use"));
    }

    #[test]
    fn note_lists_candidates_alphabetically_and_suggests() {
        let note = candidates_note("improt", "modules", &["symbol", "import", "call"]);
        assert_eq!(
            note,
            "known modules: call, import, symbol (did you mean `import`?)"
        );
    }

    #[test]
    fn note_omits_the_suggestion_when_nothing_is_close() {
        let note = candidates_note("xyzzy", "languages", &["go", "rust"]);
        assert_eq!(note, "known languages: go, rust");
    }

    #[test]
    fn note_handles_an_empty_registry() {
        assert_eq!(candidates_note("go", "languages", &[]), "no languages are registered");
    }
}
