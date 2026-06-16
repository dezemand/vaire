//! The domain model — deliberately **type-agnostic** (design.md §9).
//!
//! Vairë is a generic frontmatter-graph indexer. A node is
//! `{id, type, frontmatter, prose, outbound refs}`, where `type` is just the prefix
//! of the ID. The entity-vs-record distinction lives in the corpus *conventions* and
//! the authoring contract — never in these types. Nothing here knows what a "person"
//! or a "record" is; it only knows IDs, references, and edges.

pub mod edge;
pub mod id;
pub mod node;
pub mod reference;

pub use edge::{Edge, RefOrigin};
pub use id::{NodeId, NodeType};
pub use node::Node;
pub use reference::Reference;
