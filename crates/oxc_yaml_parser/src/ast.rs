//! AST node definitions.
//!
//! Node naming follows [yaml-unist-parser](https://github.com/prettier/yaml-unist-parser)'s unist AST
//! The shapes themselves follow this crate's own span principles:
//!
//! - Child spans nest inside their parent's span; [`Node`] wraps properties
//!   and content so anchors/tags are inside the node they apply to.
//! - A span is the construct's full lexical extent: indicators (`?`, `:`)
//!   are inside the wrapper they introduce, and a wrapper without source
//!   evidence is `None` rather than an empty-span node.
//! - Token extent and semantic boundaries are separate data
//!   ([`BlockScalar::span`] vs `content_start`/`content_end`).
//!
//! Two further deliberate departures from yaml-unist-parser:
//! - Scalar nodes do not carry cooked values; consumers slice the original source through [`Span`]s.
//! - Comments are not attached to nodes
//!   (yaml-unist-parser's leading/middle/trailing/end comment fields have no counterpart here).
//!   They live in [`Root::comments`] in source order; consumers place them positionally via spans.

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

/// A node's properties (`&anchor` / `!tag`), in either source order.
#[derive(Clone, Copy, Debug)]
pub struct Props {
    pub anchor: Option<Anchor>,
    pub tag: Option<Tag>,
}

impl Props {
    /// Start of the first property, if any.
    pub fn start(&self) -> Option<u32> {
        match (self.anchor, self.tag) {
            (Some(a), Some(t)) => Some(a.span.start.min(t.span.start)),
            (Some(a), None) => Some(a.span.start),
            (None, Some(t)) => Some(t.span.start),
            (None, None) => None,
        }
    }
}

/// The whole stream.
#[derive(Debug)]
pub struct Root<'a> {
    pub span: Span,
    pub children: Vec<'a, Document<'a>>,
    /// Every comment in the stream, in source order. Comments are not
    /// attached to nodes; consumers place them positionally via spans
    /// (the comment-cursor pattern).
    pub comments: Vec<'a, Comment>,
}

#[derive(Debug)]
#[expect(clippy::struct_field_names)] // mirrors yaml-unist-parser's field names
pub struct Document<'a> {
    pub span: Span,
    pub head: DocumentHead<'a>,
    pub body: DocumentBody<'a>,
    /// Span of the `---` marker if present.
    pub directives_end_marker: Option<Span>,
    /// Span of the `...` marker if present.
    pub document_end_marker: Option<Span>,
}

#[derive(Debug)]
pub struct DocumentHead<'a> {
    pub span: Span,
    pub directives: Vec<'a, Directive<'a>>,
}

#[derive(Debug)]
pub struct DocumentBody<'a> {
    pub span: Span,
    pub content: Option<Box<'a, Node<'a>>>,
}

/// `%NAME param param`. Uninterpreted; `%YAML`/`%TAG`/unknown are all accepted.
#[derive(Debug)]
pub struct Directive<'a> {
    pub span: Span,
    pub name: &'a str,
    pub parameters: Vec<'a, &'a str>,
}

/// A YAML node: optional properties (anchor/tag) plus the content they apply to
/// (the spec's `node ::= properties? content` production).
///
/// `span` covers the props through the content end, so every child span
/// (props, content, nested nodes) nests inside it.
///
/// Node positions hold `Box<Node>` so container children
/// ([`MappingItem`] / [`SequenceItem`]) stay small in their `Vec`s;
/// the rarely-present `Props` would otherwise inflate every element.
#[derive(Debug)]
pub struct Node<'a> {
    pub span: Span,
    pub props: Props,
    pub content: Content<'a>,
}

/// A node's content (mirrors yaml-unist-parser's `ContentNode`).
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
}

/// A plain (unquoted) scalar. `span` covers the raw scalar text
/// (trailing whitespace/comments excluded).
#[derive(Debug)]
pub struct Plain {
    pub span: Span,
}

/// `'...'`. `span` includes the quotes.
#[derive(Debug)]
pub struct QuoteSingle {
    pub span: Span,
}

/// `"..."`. `span` includes the quotes.
#[derive(Debug)]
pub struct QuoteDouble {
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
/// line breaks consumed while scanning — they are the token's lexical extent,
/// and under keep chomping part of the value).
#[derive(Debug)]
pub struct BlockScalar {
    pub span: Span,
    pub chomping: Chomping,
    /// Explicit indentation indicator digit, if any.
    pub indent: Option<u32>,
    /// Offset right after the header line's line break (= where content
    /// scanning began). The content text is `content_start..content_end`.
    pub content_start: u32,
    /// Offset right after the last content character (before the trailing
    /// break run). `content_end..span.end` holds only line breaks and
    /// blank-line indentation.
    pub content_end: u32,
}

/// A block mapping.
#[derive(Debug)]
pub struct Mapping<'a> {
    pub span: Span,
    pub children: Vec<'a, MappingItem<'a>>,
}

/// One `key: value` pair in a block or flow mapping.
#[derive(Debug)]
pub struct MappingItem<'a> {
    pub span: Span,
    /// `None` when the source has neither a `?` indicator nor key content (`: value`).
    pub key: Option<MappingKey<'a>>,
    /// `None` when the source has no `:` (`? key` alone, or a lone key in a flow mapping).
    pub value: Option<MappingValue<'a>>,
}

impl<'a> MappingItem<'a> {
    /// The key's content node, when both the key and its content exist.
    pub fn key_content(&self) -> Option<&Node<'a>> {
        self.key.as_ref().and_then(|key| key.content.as_deref())
    }

    /// The value's content node, when both the value and its content exist.
    pub fn value_content(&self) -> Option<&Node<'a>> {
        self.value.as_ref().and_then(|value| value.content.as_deref())
    }
}

/// A mapping key. `span` starts at the `?` indicator when explicit.
#[derive(Debug)]
pub struct MappingKey<'a> {
    pub span: Span,
    /// `None` for an explicit `?` with no content.
    pub content: Option<Box<'a, Node<'a>>>,
    /// `true` when written with the explicit `?` indicator.
    pub explicit: bool,
}

/// A mapping value. `span` starts at the `:` indicator.
#[derive(Debug)]
pub struct MappingValue<'a> {
    pub span: Span,
    /// `None` for `key:` with no value.
    pub content: Option<Box<'a, Node<'a>>>,
}

/// A block sequence.
#[derive(Debug)]
pub struct Sequence<'a> {
    pub span: Span,
    pub children: Vec<'a, SequenceItem<'a>>,
}

/// One `- item` in a block sequence. `span` starts at the `-`.
#[derive(Debug)]
pub struct SequenceItem<'a> {
    pub span: Span,
    pub content: Option<Box<'a, Node<'a>>>,
}

/// `{ ... }`.
#[derive(Debug)]
pub struct FlowMapping<'a> {
    pub span: Span,
    pub children: Vec<'a, MappingItem<'a>>,
}

/// `[ ... ]`.
#[derive(Debug)]
pub struct FlowSequence<'a> {
    pub span: Span,
    pub children: Vec<'a, FlowSequenceEntry<'a>>,
}

/// An entry in a flow sequence: a plain node, or a `key: value` pair.
/// Both are boxed so the enum stays two words.
#[derive(Debug)]
pub enum FlowSequenceEntry<'a> {
    Item(Box<'a, Node<'a>>),
    Pair(Box<'a, MappingItem<'a>>),
}

/// `*name`. `span` covers the `*` and the name.
#[derive(Debug)]
pub struct Alias {
    pub span: Span,
}
