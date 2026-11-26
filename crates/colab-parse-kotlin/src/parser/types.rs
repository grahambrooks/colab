//! Types: user types and their arguments, nullable, definitely-non-null,
//! function and parenthesized types; type parameters and constraints.

use super::{Item, Parser, node};
use crate::lexer::L;

impl Parser<'_> {
    /// A type.
    pub(crate) fn type_(&mut self) -> Item {
        self.type_until(None)
    }

    /// A function's or property's receiver type, which ends before the
    /// `.` at `dot`.
    pub(crate) fn receiver_type(&mut self, dot: usize) -> Item {
        self.type_until(Some(dot))
    }

    fn type_modifiers_here(&self) -> bool {
        self.at("@")
            || (self.at("suspend")
                && matches!(self.kind_at(self.i + 1), L::Word | L::Op("(" | "@")))
    }

    fn type_until(&mut self, limit: Option<usize>) -> Item {
        // A function type's result leaves its `?` to the function type.
        let allow_q = !std::mem::take(&mut self.no_nullable);
        let mods = if self.type_modifiers_here() {
            let mut m = Vec::new();
            while self.type_modifiers_here() {
                m.push(if self.at("@") {
                    self.annotation()
                } else {
                    self.punct()
                });
            }
            Some(node("type_modifiers", m))
        } else {
            None
        };
        let mut t = if self.at("(") {
            let end = self.scan_balanced(self.i).unwrap_or(self.i + 1);
            if self.is(end, "->") {
                self.function_type(None, mods.clone())
            } else {
                let items = vec![self.punct(), self.blind(Self::type_), self.expect(")")];
                let p = node("parenthesized_type", items);
                with_modifiers(mods.clone(), p, "parenthesized_type")
            }
        } else if self.at("dynamic") {
            self.punct()
        } else {
            let u = self.user_type_limited(limit, mods.clone());
            if let Some(m) = &mods
                && u.kind() != "user_type"
            {
                let _ = m;
            }
            u
        };
        loop {
            if limit.is_some_and(|l| self.i >= l) {
                return t;
            }
            if self.at("?") && !self.nl_at(self.i) && allow_q {
                let q = self.punct();
                // `suspend () -> T?`: the nullable type holds the modifiers.
                let mut items = Vec::new();
                if let Item::Node(n) = &mut t
                    && n.kind == "function_type"
                    && n.items
                        .first()
                        .is_some_and(|i| i.kind() == "type_modifiers")
                {
                    items.push(n.items.remove(0));
                }
                items.push(t);
                items.push(q);
                t = node("nullable_type", items);
                continue;
            }
            if self.at("&") && !self.is(self.i + 1, "&") {
                let amp = self.punct();
                let r = self.type_until(limit);
                t = node("non_nullable_type", vec![t, amp, r]);
                continue;
            }
            // `A.() -> B`: a function type with a receiver.
            if self.at(".") && self.is(self.i + 1, "(") {
                let end = self.scan_balanced(self.i + 1).unwrap_or(0);
                if self.is(end, "->") {
                    let dot = self.punct();
                    t = self.function_type(Some((t, dot)), None);
                    continue;
                }
            }
            return t;
        }
    }

    fn function_type(&mut self, receiver: Option<(Item, Item)>, mods: Option<Item>) -> Item {
        let mut items = Vec::new();
        if let Some(m) = mods {
            items.push(m);
        }
        if let Some((r, dot)) = receiver {
            items.push(r);
            items.push(dot);
        }
        let mut params = vec![self.punct()];
        self.blind(|p| {
            p.comma_list(&mut params, ")", |p| {
                if p.kind_at(p.i) == L::Word && p.is(p.i + 1, ":") && !p.is(p.i + 1, "::") {
                    p.parameter()
                } else {
                    p.type_()
                }
            });
        });
        params.push(self.expect(")"));
        items.push(node("function_type_parameters", params));
        items.push(self.expect("->"));
        self.no_nullable = true;
        items.push(self.type_());
        node("function_type", items)
    }

    /// A user type, `A.B<C>.D`.
    pub(crate) fn user_type_only(&mut self) -> Item {
        self.user_type_limited(None, None)
    }

    fn user_type_limited(&mut self, limit: Option<usize>, mods: Option<Item>) -> Item {
        let mut items = Vec::new();
        if let Some(m) = mods {
            items.push(m);
        }
        items.push(self.identifier());
        loop {
            if self.at("<")
                && let Some(_) = self.scan_type_arguments(self.i)
            {
                items.push(self.type_arguments());
            }
            if self.at(".")
                && self.kind_at(self.i + 1) == L::Word
                && limit.is_none_or(|l| self.i < l)
            {
                items.push(self.punct());
                items.push(self.identifier());
                continue;
            }
            break;
        }
        node("user_type", items)
    }

    /// Past the type arguments at `n` (`<`), if they are.
    pub(crate) fn scan_type_arguments(&self, n: usize) -> Option<usize> {
        let mut depth = 0usize;
        let mut k = n;
        loop {
            match self.kind_at(k) {
                L::Op("<") => depth += 1,
                L::Op(">") => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(k + 1);
                    }
                }
                L::Word | L::Op("." | "," | "?" | "*" | "(" | ")" | "->" | ":" | "@" | "&") => {}
                _ => return None,
            }
            k += 1;
        }
    }

    pub(crate) fn type_arguments(&mut self) -> Item {
        let mut items = vec![self.punct()];
        self.blind(|p| {
            loop {
                let before = p.i;
                let mut proj = Vec::new();
                if p.at("*") {
                    proj.push(p.punct());
                } else {
                    while matches!(p.word(), "in" | "out")
                        && matches!(p.kind_at(p.i + 1), L::Word | L::Op("(" | "@"))
                    {
                        proj.push(p.wrapped("variance_modifier"));
                    }
                    proj.push(p.type_());
                }
                items.push(node("type_projection", proj));
                if p.at(",") {
                    items.push(p.punct());
                    continue;
                }
                if p.i == before && !p.at(">") && !p.eof() {
                    items.push(p.error_token());
                    continue;
                }
                break;
            }
        });
        items.push(self.expect(">"));
        node("type_arguments", items)
    }

    pub(crate) fn type_parameters(&mut self) -> Item {
        let mut items = vec![self.punct()];
        self.blind(|p| {
            p.comma_list(&mut items, ">", |p| {
                let mut t = Vec::new();
                let mut mods = Vec::new();
                loop {
                    if p.at("@") {
                        mods.push(p.annotation());
                    } else if p.at("reified") && p.kind_at(p.i + 1) == L::Word {
                        // `reified` is the modifier's own token.
                        mods.push(p.leaf("reification_modifier"));
                    } else if matches!(p.word(), "in" | "out") && p.kind_at(p.i + 1) == L::Word {
                        mods.push(p.wrapped("variance_modifier"));
                    } else {
                        break;
                    }
                }
                if !mods.is_empty() {
                    t.push(node("type_parameter_modifiers", mods));
                }
                t.push(p.identifier());
                if p.at(":") {
                    t.push(p.punct());
                    t.push(p.type_());
                }
                node("type_parameter", t)
            });
        });
        items.push(self.expect(">"));
        node("type_parameters", items)
    }

    pub(crate) fn type_constraints(&mut self) -> Item {
        let mut items = vec![self.punct()];
        loop {
            let mut c = Vec::new();
            while self.at("@") {
                c.push(self.annotation());
            }
            c.push(self.identifier());
            c.push(self.expect(":"));
            c.push(self.type_());
            items.push(node("type_constraint", c));
            if self.at(",") {
                items.push(self.punct());
                continue;
            }
            break;
        }
        node("type_constraints", items)
    }
}

/// `t` (of `kind`) with type modifiers before it, as the grammar puts them
/// in the node.
fn with_modifiers(mods: Option<Item>, t: Item, kind: &'static str) -> Item {
    match (mods, t) {
        (Some(m), Item::Node(mut n)) if n.kind == kind => {
            n.items.insert(0, m);
            Item::Node(n)
        }
        (_, t) => t,
    }
}
