//! The parser: consumes scanner tokens and builds the AST.
//!
//! The scanner (libyaml-style) already synthesizes `Block*Start`/`BlockEnd`
//! and implicit `FlowMapping*` tokens, so this layer is a straightforward
//! recursive descent over a well-structured token stream.

use crate::{
    ast::*,
    error::{Error, ErrorKind},
    pos::Span,
    scanner::{ScalarStyle, Scanner, Token, TokenKind},
};
use oxc_allocator::{Allocator, Box, Vec};

type ParseResult<T> = Result<T, Error>;

pub struct Parser<'a> {
    allocator: &'a Allocator,
    source: &'a str,
    scanner: Scanner<'a>,
    peeked: Option<Token>,
}

impl<'a> Parser<'a> {
    pub fn new(allocator: &'a Allocator, source: &'a str) -> Self {
        Self { allocator, source, scanner: Scanner::new(allocator, source), peeked: None }
    }

    /// Parse the source into a [`Root`].
    ///
    /// # Errors
    /// Returns the first syntax error encountered. No partial AST is produced.
    #[expect(clippy::cast_possible_truncation)] // guarded right below
    pub fn parse(mut self) -> ParseResult<Root<'a>> {
        if u32::try_from(self.source.len()).is_err() {
            return Err(Error::new(ErrorKind::SourceTooLong, Span::empty(0)));
        }
        let source_len = self.source.len() as u32;

        let first = self.next()?;
        debug_assert!(first.kind == TokenKind::StreamStart);

        let mut children = Vec::new_in(&self.allocator);
        loop {
            if self.peek()?.kind == TokenKind::StreamEnd {
                break;
            }
            children.push(self.parse_document()?);
        }

        // The scanner collected comments directly in the arena; move them out.
        let comments = std::mem::replace(&mut self.scanner.comments, Vec::new_in(&self.allocator));

        Ok(Root { span: Span::new(0, source_len), children, comments })
    }

    // ---------------------------------------------------------------- tokens

    fn next(&mut self) -> ParseResult<Token> {
        if let Some(t) = self.peeked.take() {
            return Ok(t);
        }
        self.scanner.next_token()?.ok_or_else(|| {
            Error::point(ErrorKind::UnexpectedEof, self.source.len().saturating_sub(1))
        })
    }

    fn peek(&mut self) -> ParseResult<&Token> {
        if self.peeked.is_none() {
            self.peeked = Some(self.next()?);
        }
        Ok(self.peeked.as_ref().unwrap())
    }

    fn peek_kind(&mut self) -> ParseResult<TokenKind> {
        Ok(self.peek()?.kind)
    }

    fn eat(&mut self, kind: TokenKind) -> ParseResult<Option<Token>> {
        if self.peek()?.kind == kind {
            return Ok(Some(self.next()?));
        }
        Ok(None)
    }

    fn alloc<T>(&self, value: T) -> Box<'a, T> {
        Box::new_in(value, &self.allocator)
    }

    /// Parse a node if the next token can start one, else `None`.
    /// `allow_indentless` also accepts a bare `BlockEntry` (an indentless
    /// sequence — only valid in mapping key/value position).
    fn parse_optional_node(&mut self, allow_indentless: bool) -> ParseResult<Option<Content<'a>>> {
        let kind = self.peek_kind()?;
        let starts =
            if allow_indentless { kind.starts_mapping_entry_node() } else { kind.starts_node() };
        if starts { Ok(Some(self.parse_node(allow_indentless)?)) } else { Ok(None) }
    }

    // -------------------------------------------------------------- documents

    fn parse_document(&mut self) -> ParseResult<Document<'a>> {
        let head_start = self.peek()?.span.start;
        let mut directives = Vec::new_in(&self.allocator);
        while self.peek_kind()? == TokenKind::Directive {
            let token = self.next()?;
            directives.push(self.build_directive(token));
        }
        let head_end = directives.last().map_or(head_start, |d: &Directive<'a>| d.span.end);

        let directives_end_marker = self.eat(TokenKind::DocumentStart)?.map(|t| t.span);
        if !directives.is_empty() && directives_end_marker.is_none() {
            return Err(Error::new(
                ErrorKind::ExpectedDocumentStart,
                Span::new(head_start, head_end),
            ));
        }

        let head = DocumentHead {
            span: Span::new(head_start, directives_end_marker.map_or(head_end, |s| s.end)),
            directives,
        };

        let content = self.parse_optional_node(false)?;

        let body_span = content.as_ref().map_or_else(
            || Span::empty(directives_end_marker.map_or(head_start, |s| s.end)),
            Content::span,
        );
        let body = DocumentBody { span: body_span, content };

        let document_end_marker = self.eat(TokenKind::DocumentEnd)?.map(|t| t.span);
        // Without an explicit `...`, the next document must be introduced by
        // `---` or directives (or the stream must end); after a `...`, a bare
        // document may follow, so anything goes.
        if document_end_marker.is_none() {
            match self.peek_kind()? {
                TokenKind::StreamEnd | TokenKind::DocumentStart | TokenKind::Directive => {}
                _ => {
                    let span = self.peek()?.span;
                    return Err(Error::new(ErrorKind::ExpectedDocumentEnd, span));
                }
            }
        }

        // The head is peeked first, so `head_start` is the document start; the
        // body/head end is the document end unless a `...` marker follows.
        let span_end = document_end_marker.map_or(body.span.end.max(head.span.end), |s| s.end);

        Ok(Document {
            span: Span::new(head_start, span_end),
            head,
            body,
            directives_end_marker,
            document_end_marker,
        })
    }

    fn build_directive(&self, token: Token) -> Directive<'a> {
        let text = token.span.slice(self.source);
        let mut words = text.trim_start_matches('%').split_ascii_whitespace();
        let name = words.next().unwrap_or("");
        let parameters = Vec::from_iter_in(words, &self.allocator);
        Directive { span: token.span, name, parameters }
    }

    // ------------------------------------------------------------------ nodes

    fn parse_props(&mut self) -> ParseResult<Props> {
        let mut props = Props { anchor: None, tag: None };
        loop {
            match self.peek_kind()? {
                TokenKind::Anchor => {
                    let token = self.next()?;
                    if props.anchor.is_some() {
                        return Err(Error::new(ErrorKind::DuplicatedNodeProperty, token.span));
                    }
                    props.anchor = Some(Anchor { span: token.span });
                }
                TokenKind::Tag => {
                    let token = self.next()?;
                    if props.tag.is_some() {
                        return Err(Error::new(ErrorKind::DuplicatedNodeProperty, token.span));
                    }
                    props.tag = Some(Tag { span: token.span });
                }
                _ => break,
            }
        }
        Ok(props)
    }

    /// `allow_indentless`: whether a bare `BlockEntry` after the props starts
    /// an indentless sequence. Only mapping key/value position allows one
    /// (YAML `seq-spaces`); in sequence-item position `- !!tag\n- next` is an
    /// empty tagged node followed by the parent's next entry.
    fn parse_node(&mut self, allow_indentless: bool) -> ParseResult<Content<'a>> {
        let props = self.parse_props()?;

        let token = *self.peek()?;
        match token.kind {
            TokenKind::Alias => {
                self.next()?;
                if props.anchor.is_some() || props.tag.is_some() {
                    return Err(Error::new(ErrorKind::DuplicatedNodeProperty, token.span));
                }
                Ok(Content::Alias(self.alloc(Alias { span: token.span, props })))
            }
            TokenKind::Scalar(style, header_index) => {
                self.next()?;
                Ok(self.build_scalar(props, style, header_index, token.span))
            }
            TokenKind::FlowSequenceStart => self.parse_flow_sequence(props),
            TokenKind::FlowMappingStart => self.parse_flow_mapping(props),
            TokenKind::BlockSequenceStart => self.parse_block_sequence(props),
            TokenKind::BlockMappingStart => self.parse_block_mapping(props),
            // A `BlockEntry` with no preceding `BlockSequenceStart` is an
            // indentless sequence (a sequence at the same indentation as its
            // parent mapping key: `key:\n- a`). Outside key/value position it
            // falls through to the empty-node synthesis below.
            TokenKind::BlockEntry if allow_indentless => self.parse_indentless_sequence(props),
            _ => {
                // Properties with no following content (e.g. `!!str : v`):
                // synthesize an empty plain scalar carrying the properties.
                if props.anchor.is_some() || props.tag.is_some() {
                    let at = props
                        .anchor
                        .map(|a| a.span.end)
                        .max(props.tag.map(|t| t.span.end))
                        .unwrap();
                    return Ok(Content::Plain(self.alloc(Plain { span: Span::empty(at), props })));
                }
                Err(Error::new(ErrorKind::ExpectedNode, token.span))
            }
        }
    }

    fn build_scalar(
        &self,
        props: Props,
        style: ScalarStyle,
        header_index: Option<crate::scanner::BlockHeaderIndex>,
        span: Span,
    ) -> Content<'a> {
        match style {
            ScalarStyle::Plain => Content::Plain(self.alloc(Plain { span, props })),
            ScalarStyle::SingleQuoted => {
                Content::QuoteSingle(self.alloc(QuoteSingle { span, props }))
            }
            ScalarStyle::DoubleQuoted => {
                Content::QuoteDouble(self.alloc(QuoteDouble { span, props }))
            }
            ScalarStyle::Literal | ScalarStyle::Folded => {
                let index = header_index.expect("block scalar token must carry a header index");
                let header = self.scanner.block_headers[index.get()];
                let node = BlockScalar {
                    span,
                    props,
                    chomping: header.chomping,
                    indent: header.indent,
                    content_start: header.content_start,
                };
                if style == ScalarStyle::Literal {
                    Content::BlockLiteral(self.alloc(node))
                } else {
                    Content::BlockFolded(self.alloc(node))
                }
            }
        }
    }

    /// Parse one `- item`. The cursor must be at a `BlockEntry` token.
    fn parse_sequence_item(&mut self) -> ParseResult<SequenceItem<'a>> {
        let entry_token = self.next()?;
        debug_assert!(entry_token.kind == TokenKind::BlockEntry);
        let content = self.parse_optional_node(false)?;
        let end = content.as_ref().map_or(entry_token.span.end, |c| c.span().end);
        Ok(SequenceItem { span: Span::new(entry_token.span.start, end), content })
    }

    fn parse_block_sequence(&mut self, props: Props) -> ParseResult<Content<'a>> {
        let start_token = self.next()?; // BlockSequenceStart
        let mut children = Vec::new_in(&self.allocator);

        loop {
            match self.peek_kind()? {
                TokenKind::BlockEnd => {
                    self.next()?;
                    break;
                }
                TokenKind::BlockEntry => children.push(self.parse_sequence_item()?),
                _ => {
                    let span = self.peek()?.span;
                    return Err(Error::new(ErrorKind::UnexpectedToken("token in sequence"), span));
                }
            }
        }

        let span = container_span(start_token.span, children.first(), children.last());
        Ok(Content::Sequence(self.alloc(Sequence { span, props, children })))
    }

    /// Parse an indentless sequence: `BlockEntry` items with no enclosing
    /// `BlockSequenceStart`/`BlockEnd` (the scanner does not roll an indent
    /// for a sequence at the same indentation as its parent mapping key).
    /// Terminates at the first token that is not a `BlockEntry`.
    fn parse_indentless_sequence(&mut self, props: Props) -> ParseResult<Content<'a>> {
        let mut children = Vec::new_in(&self.allocator);
        let first = self.peek()?.span;

        while self.peek_kind()? == TokenKind::BlockEntry {
            children.push(self.parse_sequence_item()?);
        }

        let span = container_span(Span::empty(first.start), children.first(), children.last());
        Ok(Content::Sequence(self.alloc(Sequence { span, props, children })))
    }

    fn parse_block_mapping(&mut self, props: Props) -> ParseResult<Content<'a>> {
        let start_token = self.next()?; // BlockMappingStart
        let mut children = Vec::new_in(&self.allocator);

        loop {
            match self.peek_kind()? {
                TokenKind::BlockEnd => {
                    self.next()?;
                    break;
                }
                TokenKind::Key | TokenKind::Value => {
                    children.push(self.parse_mapping_item()?);
                }
                _ => {
                    let span = self.peek()?.span;
                    return Err(Error::new(ErrorKind::UnexpectedToken("token in mapping"), span));
                }
            }
        }

        let span = container_span(start_token.span, children.first(), children.last());
        Ok(Content::Mapping(self.alloc(Mapping { span, props, children })))
    }

    /// Parse one `key: value` pair (block or flow; the token structure is the
    /// same). The cursor must be at a `Key` or `Value` token.
    fn parse_mapping_item(&mut self) -> ParseResult<MappingItem<'a>> {
        let key = if let Some(key_token) = self.eat(TokenKind::Key)? {
            // A real `Key` token is always a literal `?`; a synthesized one is
            // the scanner's retroactive marker for an implicit `key:`.
            let explicit = !key_token.synthesized;
            let content = self.parse_optional_node(true)?;
            let span = content.as_ref().map_or(Span::empty(key_token.span.start), Content::span);
            MappingKey { span, content, explicit }
        } else {
            // A `Value` with no preceding `Key` (`: value`).
            let at = self.peek()?.span.start;
            MappingKey { span: Span::empty(at), content: None, explicit: false }
        };

        let value = if let Some(value_token) = self.eat(TokenKind::Value)? {
            let content = self.parse_optional_node(true)?;
            let span = content.as_ref().map_or(Span::empty(value_token.span.end), Content::span);
            MappingValue { span, content }
        } else {
            // Key with no value (`key:` is Key+Value; a lone key inside a flow
            // mapping like `{a}` has no Value token).
            MappingValue { span: Span::empty(key.span.end), content: None }
        };

        let span = Span::new(key.span.start, value.span.end.max(key.span.end));
        Ok(MappingItem { span, key, value })
    }

    fn parse_flow_sequence(&mut self, props: Props) -> ParseResult<Content<'a>> {
        let start_token = self.next()?; // FlowSequenceStart
        let mut children = Vec::new_in(&self.allocator);

        loop {
            match self.peek_kind()? {
                TokenKind::FlowSequenceEnd => {
                    let end_token = self.next()?;
                    let span = Span::new(start_token.span.start, end_token.span.end);
                    return Ok(Content::FlowSequence(self.alloc(FlowSequence {
                        span,
                        props,
                        children,
                    })));
                }
                TokenKind::FlowEntry => {
                    self.next()?;
                }
                TokenKind::Key | TokenKind::Value => {
                    // An explicit pair (`[? a: b]`), or an implicit pair for
                    // which the scanner did not synthesize a `FlowMappingStart`.
                    let item = self.parse_mapping_item()?;
                    children.push(FlowSequenceEntry::Pair(self.alloc(item)));
                }
                _ => {
                    // An implicit single pair (`[a: b]`) is surfaced by the
                    // scanner as a synthesized `FlowMappingStart` wrapper.
                    let is_synthesized_pair = {
                        let token = self.peek()?;
                        token.kind == TokenKind::FlowMappingStart && token.synthesized
                    };
                    let content = self.parse_node(false)?;
                    if is_synthesized_pair {
                        if let Content::FlowMapping(mapping) = content {
                            let mut mapping = mapping.unbox();
                            debug_assert!(mapping.children.len() == 1);
                            if let Some(item) = mapping.children.pop() {
                                children.push(FlowSequenceEntry::Pair(self.alloc(item)));
                            }
                            continue;
                        }
                        unreachable!("synthesized FlowMappingStart must produce a FlowMapping");
                    }
                    let span = content.span();
                    children.push(FlowSequenceEntry::Item(FlowSequenceItem { span, content }));
                }
            }
        }
    }

    fn parse_flow_mapping(&mut self, props: Props) -> ParseResult<Content<'a>> {
        let start_token = self.next()?; // FlowMappingStart
        let mut children = Vec::new_in(&self.allocator);

        loop {
            match self.peek_kind()? {
                TokenKind::FlowMappingEnd => {
                    let end_token = self.next()?;
                    let span = if end_token.synthesized {
                        // Synthesized end of an implicit mapping.
                        container_span(start_token.span, children.first(), children.last())
                    } else {
                        Span::new(start_token.span.start, end_token.span.end)
                    };
                    return Ok(Content::FlowMapping(self.alloc(FlowMapping {
                        span,
                        props,
                        children,
                    })));
                }
                TokenKind::FlowEntry => {
                    self.next()?;
                }
                TokenKind::Key | TokenKind::Value => {
                    children.push(self.parse_mapping_item()?);
                }
                _ if self.peek_kind()?.starts_node() => {
                    // A lone node in a flow mapping: `{a}` = `{a: null}`.
                    let content = self.parse_node(false)?;
                    let span = content.span();
                    children.push(MappingItem {
                        span,
                        key: MappingKey { span, content: Some(content), explicit: false },
                        value: MappingValue { span: Span::empty(span.end), content: None },
                    });
                }
                _ => {
                    let span = self.peek()?.span;
                    return Err(Error::new(
                        ErrorKind::UnexpectedToken("token in flow mapping"),
                        span,
                    ));
                }
            }
        }
    }
}

/// Span of a container from its (possibly empty) start token and its first and
/// last children (children are in source order).
fn container_span<T: HasSpan>(start: Span, first: Option<&T>, last: Option<&T>) -> Span {
    let start_pos = first.map_or(start.start, |c| c.span().start.min(start.start));
    let end_pos = last.map_or(start.end, |c| c.span().end.max(start.end));
    Span::new(start_pos, end_pos)
}

/// Internal helper for [`container_span`].
trait HasSpan {
    fn span(&self) -> Span;
}

impl HasSpan for SequenceItem<'_> {
    fn span(&self) -> Span {
        self.span
    }
}

impl HasSpan for MappingItem<'_> {
    fn span(&self) -> Span {
        self.span
    }
}
