//! 26.3 data model: strict JSON parsing and typed density-function nodes.
//!
//! Layered: [`json`] is the strict value parser (duplicate-key rejection,
//! depth budget, path-diagnosing errors); [`node`] is the version's typed
//! node schema. Linking (references → closed trees) lives in
//! [`crate::link`]; evaluation in [`crate::eval`].

pub mod json;
pub mod node;

pub use json::{parse_strict, Json, JsonError, MAX_JSON_DEPTH};
pub use node::{parse_node, Axis, Node, ParseError, Spline};
