//! Reference syntax and its one load-bearing parse rule (design.md §6).
//!
//! A reference is a `[[...]]` wikilink in one of three states:
//!
//! ```text
//! [[person:jane-doe]]                              resolved (ID known)
//! [[person:jane-doe|Jane]]                         resolved, with display text
//! [[?person: someone from logistics]]              unresolved (type guess + descriptor)
//! [[?: the broker thing]]                          unresolved, type unknown
//! [[person:logistics-contact|someone from ...]]    resolved-from-unresolved (phrasing kept)
//! ```
//!
//! **The parse rule:** a `?` immediately after `[[` means **unresolved**. Everything
//! after the colon is then a *descriptor*, never an ID — and the index must **not**
//! follow it as a graph edge (cli.md §3.3). No `?` ⇒ a real reference to a real node.
//!
//! The core safety property (design.md §6): an unresolved reference carries a
//! *description of what the author saw*, never a proposed name. There is no syntax on
//! the autonomous path for asserting an identity — so there is no slug to collide.

use crate::model::id::{NodeId, NodeType};

/// A parsed `[[...]]` reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reference {
    /// `[[type:slug]]` or `[[type:slug|display]]` — a real edge to a real node.
    Resolved {
        target: NodeId,
        /// Optional display text after `|`. For resolved-from-unresolved references,
        /// this preserves the author's original phrasing (design.md §6).
        display: Option<String>,
    },
    /// `[[?type: descriptor]]` or `[[?: descriptor]]` — a loose end. Never an edge.
    Unresolved {
        /// The optional `?type` hint. `None` for `[[?: …]]`. Overridable by the
        /// entity-creation pass (design.md §8).
        type_guess: Option<NodeType>,
        /// What the author saw — a descriptor, not a name.
        descriptor: String,
    },
}

impl Reference {
    /// True iff this reference is a graph edge (i.e. resolved). Unresolved references
    /// are explicitly *not* edges (cli.md §3.3).
    pub fn is_edge(&self) -> bool {
        matches!(self, Reference::Resolved { .. })
    }

    /// Render a **resolved** reference as a relative Markdown link `[display](href)`
    /// (design.md §6). The display text is the `|` override when present, otherwise the
    /// target's `name:` (`name`); `href` is the path to the target file. Returns `None`
    /// for unresolved references — there is nothing to link yet.
    pub fn render_link(&self, name: &str, href: &str) -> Option<String> {
        match self {
            Reference::Resolved { display, .. } => {
                let text = display.as_deref().unwrap_or(name);
                Some(format!("[{text}]({href})"))
            }
            Reference::Unresolved { .. } => None,
        }
    }

    /// Parse the inner text of a `[[...]]` (without the surrounding brackets).
    ///
    /// Returns `None` if the inner text is not a well-formed reference, so a stray
    /// `[[` in prose does not become a phantom edge.
    pub fn parse_inner(inner: &str) -> Option<Reference> {
        let inner = inner.trim();
        if let Some(rest) = inner.strip_prefix('?') {
            // Unresolved: `?type: descriptor` or `?: descriptor`.
            let (type_part, descriptor) = rest.split_once(':')?;
            let type_part = type_part.trim();
            let type_guess = if type_part.is_empty() {
                None
            } else {
                Some(NodeType::new(type_part))
            };
            let descriptor = descriptor.trim().to_string();
            if descriptor.is_empty() {
                return None;
            }
            Some(Reference::Unresolved {
                type_guess,
                descriptor,
            })
        } else {
            // Resolved: `type:slug` with optional `|display`.
            let (id_part, display) = match inner.split_once('|') {
                Some((id, disp)) => (id.trim(), Some(disp.trim().to_string())),
                None => (inner, None),
            };
            let target: NodeId = id_part.parse().ok()?;
            Some(Reference::Resolved { target, display })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_reference_renders_with_target_name() {
        // [[department:hr]] → [Human Resources](./hr.md) — display from name: (design.md §6).
        let r = Reference::parse_inner("department:hr").unwrap();
        assert_eq!(
            r.render_link("Human Resources", "./hr.md").as_deref(),
            Some("[Human Resources](./hr.md)")
        );
    }

    #[test]
    fn piped_reference_overrides_display() {
        // [[department:hr|HR]] → [HR](./hr.md) — piped text wins over name:.
        let r = Reference::parse_inner("department:hr|HR").unwrap();
        assert_eq!(
            r.render_link("Human Resources", "./hr.md").as_deref(),
            Some("[HR](./hr.md)")
        );
    }

    #[test]
    fn unresolved_reference_does_not_render() {
        let r = Reference::parse_inner("?department: the hr folks").unwrap();
        assert_eq!(r.render_link("anything", "./x.md"), None);
    }
}
