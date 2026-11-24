//! `include` and `then`: a script's items flattened into one ordered list
//! of rules, scans and stage marks, with the definitions (`pattern`,
//! `rewriter`) they make.

use super::*;

/// A script's contents in order, includes expanded: rules, scans, and
/// where a `then { … }` stage begins or ends.
pub(crate) enum Entry {
    Match(Match),
    Scan(crate::ast::Scan),
    Stage,
}

/// Recursively expand `include` directives (and `then` blocks), in source
/// order.
pub(crate) fn expand_includes(
    items: Vec<Item>,
    base_path: Option<&Path>,
    seen: &mut HashSet<PathBuf>,
    defs: &mut Definitions,
) -> Result<Vec<Entry>> {
    let mut out = Vec::new();
    for item in items {
        defs.add(&item)?;
        match item {
            Item::Match(m) => out.push(Entry::Match(m)),
            Item::Scan(scan) => out.push(Entry::Scan(scan)),
            Item::Then(inner) => {
                out.push(Entry::Stage);
                out.extend(expand_includes(inner, base_path, seen, defs)?);
                out.push(Entry::Stage);
            }
            Item::Pattern { .. } | Item::Rewriter { .. } => {}
            Item::Include(rel) => {
                let resolved = resolve_include_path(&rel, base_path)?;
                let canonical = resolved
                    .canonicalize()
                    .map_err(|e| Error::io_at(&resolved, e))?;
                if !seen.insert(canonical.clone()) {
                    return Err(Error::Config(format!(
                        "circular include: {} re-includes itself",
                        resolved.display()
                    )));
                }
                let text = fs::read_to_string(&resolved).map_err(|e| Error::io_at(&resolved, e))?;
                let inner = parse(&text).map_err(|e| in_file(e, Some(&resolved)))?;
                let nested = expand_includes(inner.items, Some(&resolved), seen, defs)?;
                out.extend(nested);
            }
        }
    }
    Ok(out)
}

/// The script's named patterns and rewriters (its includes' too).
#[derive(Default)]
pub(crate) struct Definitions {
    pub(crate) patterns: std::collections::HashMap<String, String>,
    /// Name → (pattern, template).
    pub(crate) rewriters: std::collections::HashMap<String, (String, String)>,
}

impl Definitions {
    /// Record `item` if it defines something, each name once per kind.
    pub(crate) fn add(&mut self, item: &Item) -> Result<()> {
        let twice = |kind: &str, name: &str| {
            Error::Config(format!(
                "`{kind} {name}` is defined twice; give each {kind} its own name"
            ))
        };
        match item {
            Item::Pattern { name, text, .. } => {
                if self.patterns.insert(name.clone(), text.clone()).is_some() {
                    return Err(twice("pattern", name));
                }
            }
            Item::Rewriter {
                name,
                pattern,
                template,
                ..
            } => {
                let def = (pattern.clone(), template.clone());
                if self.rewriters.insert(name.clone(), def).is_some() {
                    return Err(twice("rewriter", name));
                }
            }
            Item::Match(_) | Item::Include(_) | Item::Scan(_) => {}
            Item::Then(inner) => {
                for item in inner {
                    self.add(item)?;
                }
            }
        }
        Ok(())
    }
}

pub(crate) fn resolve_include_path(relative: &str, base_path: Option<&Path>) -> Result<PathBuf> {
    let p = Path::new(relative);
    if p.is_absolute() {
        return Ok(p.to_path_buf());
    }
    let base = base_path.ok_or_else(|| {
        Error::Config(format!(
            "cannot resolve `include \"{relative}\"`: no base path. Use `compile_at_path` instead of `compile` to enable includes."
        ))
    })?;
    let parent = base.parent().unwrap_or(Path::new("."));
    Ok(parent.join(relative))
}
