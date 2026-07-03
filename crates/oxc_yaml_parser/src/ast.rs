//! AST node definitions.
//!
//! The node shapes mirror [yaml-unist-parser](https://github.com/prettier/yaml-unist-parser)'s
//! unist AST — the AST Prettier's YAML printer consumes — to keep a
//! Prettier-compatible printer close to its reference.
//!
//! Two deliberate departures:
//! - Scalar nodes do not carry cooked values; consumers slice the original
//!   source through [`Span`]s.
//! - Comments are not attached to nodes (yaml-unist-parser's
//!   leading/middle/trailing/end comment fields have no counterpart here).
//!   They live in [`Root::comments`] in source order; consumers place them
//!   positionally via spans (the comment-cursor pattern used by the other
//!   oxc formatters).

use crate::pos::Span;
use oxc_allocator::{Box, Vec};

/// A `#` comment. `span` covers `#` through the end of the comment text.
#[derive(Clone, Copy, Debug)]
pub struct Comment {
    pub span: Span,
}

/// `&name`. `span` covers the `&` and the name.
#[derive(Clone, Copy, Debug)]
pub struct Anchor {
    pub span: Span,
}

/// A tag property: `!`, `!suffix`, `!handle!suffix`, `!!suffix` or `!<verbatim>`.
#[derive(Clone, Copy, Debug)]
pub struct Tag {
    pub span: Span,
}

/// Properties shared by every content node (mirrors yaml-unist-parser's `Content`).
#[derive(Clone, Copy, Debug)]
pub struct Props {
    pub anchor: Option<Anchor>,
    pub tag: Option<Tag>,
}

/// The whole stream.
#[derive(Debug)]
pub struct Root<'a> {
    pub children: Vec<'a, Document<'a>>,
    /// Every comment in the stream, in source order. Comments are not
    /// attached to nodes; consumers place them positionally via spans
    /// (the comment-cursor pattern).
    pub comments: Vec<'a, Comment>,
    pub span: Span,
}

#[derive(Debug)]
#[expect(clippy::struct_field_names)] // mirrors yaml-unist-parser's field names
pub struct Document<'a> {
    pub head: DocumentHead<'a>,
    pub body: DocumentBody<'a>,
    /// Span of the `---` marker if present.
    pub directives_end_marker: Option<Span>,
    /// Span of the `...` marker if present.
    pub document_end_marker: Option<Span>,
    pub span: Span,
}

#[derive(Debug)]
pub struct DocumentHead<'a> {
    pub directives: Vec<'a, Directive<'a>>,
    pub span: Span,
}

#[derive(Debug)]
pub struct DocumentBody<'a> {
    pub content: Option<Content<'a>>,
    pub span: Span,
}

/// `%NAME param param`. Uninterpreted; `%YAML`/`%TAG`/unknown are all accepted.
#[derive(Debug)]
pub struct Directive<'a> {
    pub name: &'a str,
    pub parameters: Vec<'a, &'a str>,
    pub span: Span,
}

/// A content node (mirrors yaml-unist-parser's `ContentNode`).
#[derive(Debug)]
pub enum Content<'a> {
    Plain(Box<'a, Plain>),
    QuoteSingle(Box<'a, QuoteSingle>),
    QuoteDouble(Box<'a, QuoteDouble>),
    BlockLiteral(Box<'a, BlockScalar>),
    BlockFolded(Box<'a, BlockScalar>),
    Mapping(Box<'a, Mapping<'a>>),
    Sequence(Box<'a, Sequence<'a>>),
    FlowMapping(Box<'a, FlowMapping<'a>>),
    FlowSequence(Box<'a, FlowSequence<'a>>),
    Alias(Box<'a, Alias>),
}

impl Content<'_> {
    pub fn span(&self) -> Span {
        match self {
            Content::Plain(n) => n.span,
            Content::QuoteSingle(n) => n.span,
            Content::QuoteDouble(n) => n.span,
            Content::BlockLiteral(n) | Content::BlockFolded(n) => n.span,
            Content::Mapping(n) => n.span,
            Content::Sequence(n) => n.span,
            Content::FlowMapping(n) => n.span,
            Content::FlowSequence(n) => n.span,
            Content::Alias(n) => n.span,
        }
    }

    pub fn props(&self) -> &Props {
        match self {
            Content::Plain(n) => &n.props,
            Content::QuoteSingle(n) => &n.props,
            Content::QuoteDouble(n) => &n.props,
            Content::BlockLiteral(n) | Content::BlockFolded(n) => &n.props,
            Content::Mapping(n) => &n.props,
            Content::Sequence(n) => &n.props,
            Content::FlowMapping(n) => &n.props,
            Content::FlowSequence(n) => &n.props,
            Content::Alias(n) => &n.props,
        }
    }
}

/// A plain (unquoted) scalar. `span` covers the raw scalar text
/// (trailing whitespace/comments excluded).
#[derive(Debug)]
pub struct Plain {
    pub props: Props,
    pub span: Span,
}

/// `'...'`. `span` includes the quotes.
#[derive(Debug)]
pub struct QuoteSingle {
    pub props: Props,
    pub span: Span,
}

/// `"..."`. `span` includes the quotes.
#[derive(Debug)]
pub struct QuoteDouble {
    pub props: Props,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Chomping {
    /// (default) single trailing newline
    Clip,
    /// `+` keep all trailing newlines
    Keep,
    /// `-` strip all trailing newlines
    Strip,
}

/// `|` (literal) or `>` (folded) block scalar.
///
/// The variant is distinguished by the enclosing [`Content`] variant. `span`
/// covers the indicator through the end of the content (including trailing
/// line breaks consumed while scanning).
#[derive(Debug)]
pub struct BlockScalar {
    pub props: Props,
    pub chomping: Chomping,
    /// Explicit indentation indicator digit, if any.
    pub indent: Option<u32>,
    /// Offset right after the header line's line break (= where content
    /// scanning began). The content is `content_start..span.end`.
    pub content_start: u32,
    pub span: Span,
}

/// A block mapping.
#[derive(Debug)]
pub struct Mapping<'a> {
    pub props: Props,
    pub children: Vec<'a, MappingItem<'a>>,
    pub span: Span,
}

/// One `key: value` pair in a block mapping.
#[derive(Debug)]
pub struct MappingItem<'a> {
    pub key: MappingKey<'a>,
    pub value: MappingValue<'a>,
    pub span: Span,
}

#[derive(Debug)]
pub struct MappingKey<'a> {
    /// `None` for a value-less key position (`: value` with explicit `?`, or empty).
    pub content: Option<Content<'a>>,
    /// `true` when written with the explicit `?` indicator.
    pub explicit: bool,
    pub span: Span,
}

#[derive(Debug)]
pub struct MappingValue<'a> {
    /// `None` for `key:` with no value.
    pub content: Option<Content<'a>>,
    pub span: Span,
}

/// A block sequence.
#[derive(Debug)]
pub struct Sequence<'a> {
    pub props: Props,
    pub children: Vec<'a, SequenceItem<'a>>,
    pub span: Span,
}

/// One `- item` in a block sequence. `span` starts at the `-`.
#[derive(Debug)]
pub struct SequenceItem<'a> {
    pub content: Option<Content<'a>>,
    pub span: Span,
}

/// `{ ... }`.
#[derive(Debug)]
pub struct FlowMapping<'a> {
    pub props: Props,
    pub children: Vec<'a, MappingItem<'a>>,
    pub span: Span,
}

/// `[ ... ]`.
#[derive(Debug)]
pub struct FlowSequence<'a> {
    pub props: Props,
    pub children: Vec<'a, FlowSequenceEntry<'a>>,
    pub span: Span,
}

/// An entry in a flow sequence: a plain item, or a `key: value` pair.
/// The pair is boxed so plain items don't pay for the larger variant.
#[derive(Debug)]
pub enum FlowSequenceEntry<'a> {
    Item(FlowSequenceItem<'a>),
    Pair(Box<'a, MappingItem<'a>>),
}

#[derive(Debug)]
pub struct FlowSequenceItem<'a> {
    pub content: Content<'a>,
    pub span: Span,
}

/// `*name`. `span` covers the `*` and the name.
#[derive(Debug)]
pub struct Alias {
    pub props: Props,
    pub span: Span,
}
