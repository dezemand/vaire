//! The corpus — the Git repo of Markdown that is the source of truth.
//!
//! This module reads files; it **never writes them** (design.md §9). It turns the
//! working tree into [`crate::model::Node`]s: discover the repo root, scan for
//! candidate files within the configured globs, split frontmatter from prose, parse
//! the typed `id:` (frontmatter-driven discovery), and scan prose for wikilinks and
//! section headings.

pub mod frontmatter;
pub mod repo;
pub mod scan;
pub mod section;
pub mod wikilink;

pub use repo::Repo;
pub use section::Section;
