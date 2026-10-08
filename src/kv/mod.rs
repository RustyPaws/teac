//! Valve KeyValues: lexer and parser that preserve order and duplicate keys.
//! Used for VMF, gameinfo.txt and VMT files.

mod error;
mod lexer;
mod node;
mod parser;

pub use error::{ParseError, ParseErrorKind};
pub use node::{Node, NodeList, Value};
pub use parser::{parse, parse_with, Conditions};
