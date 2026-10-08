//! JSON as System.Text.Json wrote and read it, so state files and API answers stay byte
//! for byte what existing installs and clients already have.

pub mod datetime;
pub mod dom;
pub mod element;
pub mod format;
pub mod web;

pub use format::{
    Escaping, Options, StjFormatter, format_double, format_single, quote, to_string, to_string_indented,
    to_string_with, to_vec_with,
};

/// [`to_string`] with `DefaultIgnoreCondition = JsonIgnoreCondition.WhenWritingNull`: every
/// member whose value is null is left out, at every depth. (STJ applies the condition to
/// properties only; a dictionary entry whose value is null would be kept. The files written
/// this way hold no dictionary with a null value.)
pub fn to_string_without_nulls<T: serde::Serialize + ?Sized>(value: &T) -> String {
    fn strip(node: &mut dom::Node) {
        match node {
            dom::Node::Object(members) => {
                members.retain(|_, member| !matches!(member, dom::Node::Null));
                members.values_mut().for_each(strip);
            }
            dom::Node::Array(items) => items.iter_mut().for_each(strip),
            _ => {}
        }
    }
    let written = to_string(value);
    match dom::Node::parse(&written) {
        Ok(mut node) => {
            strip(&mut node);
            node.to_json_string(false)
        }
        Err(_) => written,
    }
}
