//! Moving a section from one Markdown file to another.
//!
//! Every rule acts on one file, so a move is two rules: an `insert` of
//! the section in the target and a `delete` in the source. [`plan`] works
//! out what those rules need — the section's body, its subsection
//! headings shifted to the level it lands at, and its anchor before and
//! after — so `colab mv-section` can generate them (and rules that
//! repoint the links) and run them as one script.

use crate::outline::{self, Heading};
use crate::section::SectionEdit;
use colab_core::Operation;

/// Where a moved section goes in the target file.
#[derive(Debug, Clone, Copy)]
pub enum Placement<'a> {
    /// The end of the file, at its top level.
    End,
    /// The last child of this section.
    Under(&'a str),
    /// Just before this section, at its level.
    Before(&'a str),
}

/// What the generated rules need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SectionMove {
    /// The `markdown::section` address that inserts it in the target.
    pub insert_address: String,
    /// The section's body, its subsection headings re-levelled.
    pub body: String,
    /// Its anchor in the source, and in the target once moved.
    pub old_slug: String,
    pub new_slug: String,
    /// Its level in the source and in the target.
    pub old_level: u8,
    pub new_level: u8,
}

/// Plan moving `address` from `source` to `target` at `placement`.
pub fn plan(
    source: &str,
    address: &str,
    target: &str,
    placement: Placement<'_>,
) -> Result<SectionMove, String> {
    let headings = outline::headings(source);
    let section = outline::resolve(&headings, address)?;
    let title = source[section.content.clone()].trim().to_string();
    if title.contains(" > ") || title.contains(" < ") {
        return Err(format!(
            "the heading `{title}` holds ` > ` or ` < `, which section addresses use; rename it first"
        ));
    }
    let insert_address = match placement {
        Placement::End => title.clone(),
        Placement::Under(parent) => format!("{parent} > {title}"),
        Placement::Before(sibling) => format!("{title} < {sibling}"),
    };
    // Where it lands, found by inserting the heading alone.
    let probe = SectionEdit::insert(&insert_address, "").map_err(|e| e.to_string())?;
    let landed = probe.apply(target);
    if landed == target {
        return Err(probe.why_unchanged(target).map_or_else(
            || format!("cannot place `{title}` in the target"),
            |why| {
                if why.starts_with(crate::section::ALREADY) {
                    format!("the target already has a section `{title}` there")
                } else {
                    why
                }
            },
        ));
    }
    let lookup = insert_address
        .split(" < ")
        .next()
        .unwrap_or(&insert_address)
        .to_string();
    let after = outline::headings(&landed);
    let new = outline::find(&after, &lookup)
        .into_iter()
        .find(|h| landed[h.content.clone()].trim() == title)
        .ok_or_else(|| format!("cannot find `{title}` where it would land"))?;
    let delta = i16::from(new.level) - i16::from(section.level);
    Ok(SectionMove {
        insert_address,
        body: shifted_body(source, section, &headings, delta),
        old_slug: section.slug.clone(),
        new_slug: new.slug.clone(),
        old_level: section.level,
        new_level: new.level,
    })
}

/// `section`'s text below its heading, each ATX subsection heading moved
/// `delta` levels (kept within 1..=6). Only real headings are touched, so
/// a `#` line inside a code fence stays as it is; setext headings keep
/// their level.
fn shifted_body(source: &str, section: &Heading, headings: &[Heading], delta: i16) -> String {
    let start = section.heading_end;
    let mut body = source[start..section.end].to_string();
    if delta != 0 {
        let inside: Vec<&Heading> = headings
            .iter()
            .filter(|h| h.start >= start && h.start < section.end)
            .collect();
        for h in inside.into_iter().rev() {
            let at = h.start - start;
            let hashes = body[at..].chars().take_while(|c| *c == '#').count();
            if hashes == 0 {
                continue;
            }
            let level = (hashes as i16 + delta).clamp(1, 6) as usize;
            body.replace_range(at..at + hashes, &"#".repeat(level));
        }
    }
    body.trim_matches('\n').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOURCE: &str = "# Plan\n\n## Planned\n\n### Old item\n\nText, see [x](#keep).\n\n#### Detail\n\n```sh\n# not a heading\n```\n\n## Keep\n\nstays\n";
    const TARGET: &str = "# Done\n\n## Shipped\n\n### Earlier\n\ndone\n";

    #[test]
    fn a_section_lands_under_a_parent_with_its_subsections_relevelled() {
        let m = plan(SOURCE, "Old item", TARGET, Placement::Under("Shipped")).unwrap();
        assert_eq!(m.insert_address, "Shipped > Old item");
        assert_eq!((m.old_level, m.new_level), (3, 3));
        assert!(m.body.contains("#### Detail"), "{}", m.body);
        // To the top level of a file with a lone `#` title: level 2.
        let top = plan(SOURCE, "Old item", TARGET, Placement::End).unwrap();
        assert_eq!(top.new_level, 2);
        assert!(top.body.contains("\n### Detail"), "{}", top.body);
        assert!(
            top.body.contains("# not a heading"),
            "fences are left: {}",
            top.body
        );
        assert_eq!(
            (top.old_slug.as_str(), top.new_slug.as_str()),
            ("old-item", "old-item")
        );
    }

    #[test]
    fn a_move_that_cannot_land_says_why() {
        let target = "# Done\n\n## Old item\n";
        let e = plan(SOURCE, "Old item", target, Placement::End).unwrap_err();
        assert!(e.contains("already has a section"), "{e}");
        let e = plan(SOURCE, "Nope", TARGET, Placement::End).unwrap_err();
        assert!(e.contains("no section `Nope`"), "{e}");
        let e = plan(SOURCE, "Old item", TARGET, Placement::Under("Missing")).unwrap_err();
        assert!(e.contains("Missing"), "{e}");
    }
}
