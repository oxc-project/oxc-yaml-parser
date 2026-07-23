//! oxc-yaml-parser is a YAML 1.2 parser that produces a comment-preserving,
//! span-faithful typed AST, designed for building formatters.
//!
//! ## Basic Usage
//!
//! ```rust
//! use oxc_yaml_parser::{Allocator, Parser};
//!
//! let allocator = Allocator::default();
//! let parser = Parser::new(&allocator, "key: value # comment");
//! match parser.parse() {
//!     Ok(root) => {
//!         assert_eq!(root.children.len(), 1);
//!         assert_eq!(root.comments.len(), 1);
//!     }
//!     Err(error) => {
//!         // Syntax error with span; no partial AST is produced.
//!         println!("{error}");
//!     }
//! }
//! ```

pub mod ast;
mod error;
mod parser;
mod pos;
mod scanner;

pub use error::{Error, ErrorKind};
pub use oxc_allocator::Allocator;
pub use parser::Parser;
pub use pos::Span;

/// Size regression guards, in the spirit of oxc_ast's generated assertions:
/// enums stay pointer-sized-plus-tag, and hot token types stay small.
#[cfg(all(test, target_pointer_width = "64"))]
mod size_asserts {
    use crate::{ast, scanner};

    #[test]
    fn sizes() {
        // Scanner-side: every buffered token pays these.
        assert_eq!(size_of::<scanner::Token>(), 20);
        assert_eq!(size_of::<scanner::TokenKind>(), 8);
        // AST: `Content` is tag + arena Box, and the niche keeps `Option` free.
        assert_eq!(size_of::<ast::Content>(), 16);
        assert_eq!(size_of::<Option<ast::Content>>(), 16);
        // `Node` adds the span and props on top of the content.
        // Node-position fields box it so container children stay small (guarded below).
        assert_eq!(size_of::<ast::Node>(), 48);
        assert_eq!(size_of::<ast::MappingItem>(), 56);
        assert_eq!(size_of::<ast::SequenceItem>(), 16);
        // Flow sequence entries: both variants boxed, two words total.
        assert_eq!(size_of::<ast::FlowSequenceEntry>(), 16);
    }
}
