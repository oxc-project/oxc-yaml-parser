//! The YAML scanner (tokenizer).
//!
//! A port of the libyaml scanning algorithm (by way of
//! [saphyr](https://github.com/saphyr-rs/saphyr)), adapted to:
//!
//! - operate on byte offsets over `&str` (no buffering, no `char` ring buffer),
//! - produce span-only tokens (no cooked scalar values),
//! - retain comments as trivia (libyaml discards them),
//! - be tolerant of uninterpreted directives (Prettier-compat: unknown
//!   directives and `%YAML x.y` versions are accepted).
//!
//! The core machinery — the simple key stack, the indent stack with
//! synthesized `Block*` tokens, and the flow level with implicit flow mapping
//! states — deliberately mirrors libyaml/saphyr; do not "improve" it without
//! cross-checking against the yaml-test-suite.

// Byte offsets are bounded to u32 by the parser's up-front source-length
// check, so the `usize -> u32` casts cannot truncate. Indentation comparisons
// follow libyaml's `isize` convention (-1 = no indent); columns are small, so
// `cast_signed`/`cast_unsigned` on them are lossless.
#![expect(clippy::cast_possible_truncation)]

use crate::{
    ast::{Chomping, Comment},
    error::{Error, ErrorKind},
    pos::Span,
};
use std::{collections::VecDeque, num::NonZeroU32};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScalarStyle {
    Plain,
    SingleQuoted,
    DoubleQuoted,
    Literal,
    Folded,
}

/// 1-based index into [`Scanner::block_headers`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockHeaderIndex(NonZeroU32);

impl BlockHeaderIndex {
    fn new(zero_based: usize) -> Self {
        Self(NonZeroU32::new(zero_based as u32 + 1).unwrap())
    }

    pub fn get(self) -> usize {
        self.0.get() as usize - 1
    }
}

/// Extra information scanned from a block scalar header. Stored out-of-band
/// in [`Scanner::block_headers`] (indexed by the `Scalar` token) to keep
/// [`Token`] small — every buffered token would otherwise pay for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockScalarHeader {
    pub chomping: Chomping,
    /// Explicit indentation indicator digit, if any.
    pub indent: Option<u32>,
    /// Offset right after the header line's line break.
    pub content_start: u32,
    /// Offset right after the last content character (before the trailing break run).
    /// Equals `content_start` when the scalar has no content.
    pub content_end: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenKind {
    StreamStart,
    StreamEnd,
    /// `%NAME ...`, uninterpreted. The span excludes any trailing comment.
    Directive,
    /// `---`
    DocumentStart,
    /// `...`
    DocumentEnd,
    BlockSequenceStart,
    BlockMappingStart,
    BlockEnd,
    FlowSequenceStart,
    FlowSequenceEnd,
    FlowMappingStart,
    FlowMappingEnd,
    BlockEntry,
    FlowEntry,
    Key,
    Value,
    Alias,
    Anchor,
    Tag,
    /// A scalar. Block scalars (`Literal`/`Folded`) carry their 1-based index
    /// into [`Scanner::block_headers`] (`NonZeroU32` so the `Option` is free).
    Scalar(ScalarStyle, Option<BlockHeaderIndex>),
}

impl TokenKind {
    /// Whether a token of this kind can start a node (in any position).
    pub(crate) fn starts_node(self) -> bool {
        matches!(
            self,
            TokenKind::Scalar(..)
                | TokenKind::Alias
                | TokenKind::Anchor
                | TokenKind::Tag
                | TokenKind::FlowSequenceStart
                | TokenKind::FlowMappingStart
                | TokenKind::BlockSequenceStart
                | TokenKind::BlockMappingStart
        )
    }

    /// Whether a token of this kind can start a node in mapping key/value
    /// position, where a bare `BlockEntry` (an indentless sequence) is also
    /// allowed.
    pub(crate) fn starts_mapping_entry_node(self) -> bool {
        self.starts_node() || self == TokenKind::BlockEntry
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
    /// `true` for tokens the scanner invents (block/flow structure markers,
    /// retroactively inserted `Key`s, ...) rather than reads from the source.
    pub synthesized: bool,
}

impl Token {
    fn new(kind: TokenKind, span: Span) -> Self {
        Self { kind, span, synthesized: false }
    }

    /// A zero-width token invented by the scanner at the given offset.
    fn synthesized(kind: TokenKind, at: usize) -> Self {
        Self { kind, span: span(at, at), synthesized: true }
    }
}

/// A scalar that was scanned and may retroactively become a mapping key.
///
/// Upon scanning `a` in `a: b` we do not yet know it is a key; the token is
/// buffered and this bookkeeping records where a `Key` token would need to be
/// inserted if a `:` follows.
#[derive(Clone, Copy, Debug)]
struct SimpleKey {
    possible: bool,
    required: bool,
    /// Index of the buffered token, counting tokens already handed out.
    token_number: usize,
    /// Byte offset of the candidate key.
    pos: usize,
    line: usize,
    col: usize,
}

impl SimpleKey {
    fn new() -> Self {
        Self { possible: false, required: false, token_number: 0, pos: 0, line: 0, col: 0 }
    }
}

/// An indentation level on the stack of indentations.
#[derive(Clone, Copy, Debug)]
struct Indent {
    /// The former indentation level.
    indent: isize,
    /// Whether, upon closing, this indent generates a `BlockEnd` token.
    needs_block_end: bool,
}

/// Whether a flow sequence may hold / holds an implicit `key: value` mapping
/// (`[ a: b ]`), which requires synthesizing `FlowMapping*` tokens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ImplicitMappingState {
    Possible,
    Inside,
}

type ScanResult<T = ()> = Result<T, Error>;

fn is_blank(b: u8) -> bool {
    b == b' ' || b == b'\t'
}

fn is_break(b: u8) -> bool {
    b == b'\n' || b == b'\r'
}

/// Break or end-of-input (we use NUL as the EOF sentinel like libyaml).
fn is_breakz(b: u8) -> bool {
    is_break(b) || b == 0
}

fn is_blank_or_breakz(b: u8) -> bool {
    is_blank(b) || is_breakz(b)
}

fn is_flow(b: u8) -> bool {
    matches!(b, b',' | b'[' | b']' | b'{' | b'}')
}

fn is_alpha(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}

/// Whether a plain scalar ends at byte `b` (with lookahead `next`).
/// See 7.3.3. Plain Style. Context-sensitive: flow indicators only terminate
/// a plain scalar inside a flow collection.
#[inline]
fn ends_plain_scalar(b: u8, next: u8, in_flow: bool) -> bool {
    match b {
        b':' if is_blank_or_breakz(next) || (in_flow && is_flow(next)) => true,
        _ if in_flow && is_flow(b) => true,
        _ => false,
    }
}

fn is_anchor_char(b: u8) -> bool {
    // Any non-space, non-break, non-flow character. Multi-byte UTF-8 lead and
    // continuation bytes are all >= 0x80 and thus allowed.
    !is_blank_or_breakz(b) && !is_flow(b)
}

fn is_uri_char(b: u8) -> bool {
    is_alpha(b) || b"#;/?:@&=+$,_.!~*'()[]%".contains(&b)
}

fn is_tag_char(b: u8) -> bool {
    is_uri_char(b) && !is_flow(b) && b != b'!'
}

const BOM: &[u8] = "\u{FEFF}".as_bytes();

pub struct Scanner<'a> {
    src: &'a [u8],
    /// Current byte offset.
    pos: usize,
    /// Current line (0-based; only compared relatively).
    line: usize,
    /// Current column (0-based, counted in characters).
    col: usize,

    /// Buffered tokens not yet handed to the parser.
    tokens: VecDeque<Token>,
    /// Comments encountered so far, in source order.
    pub(crate) comments: oxc_allocator::Vec<'a, Comment>,
    /// Block scalar headers, indexed by `TokenKind::Scalar`'s payload.
    pub(crate) block_headers: Vec<BlockScalarHeader>,

    stream_start_produced: bool,
    stream_end_produced: bool,
    /// Offset at which a `:` may be adjacent to a JSON-like key in flow context.
    adjacent_value_allowed_at: usize,
    simple_key_allowed: bool,
    simple_keys: Vec<SimpleKey>,
    indent: isize,
    indents: Vec<Indent>,
    flow_level: usize,
    /// Number of tokens already handed to the parser.
    tokens_parsed: usize,
    /// Whether all characters since the last newline were whitespace.
    leading_whitespace: bool,
    flow_mapping_started: bool,
    implicit_flow_mapping_states: Vec<ImplicitMappingState>,
}

impl<'a> Scanner<'a> {
    pub fn new(allocator: &'a oxc_allocator::Allocator, source: &'a str) -> Self {
        Self {
            src: source.as_bytes(),
            pos: 0,
            line: 0,
            col: 0,
            tokens: VecDeque::new(),
            comments: oxc_allocator::Vec::new_in(&allocator),
            block_headers: Vec::new(),
            stream_start_produced: false,
            stream_end_produced: false,
            adjacent_value_allowed_at: usize::MAX,
            simple_key_allowed: true,
            simple_keys: Vec::new(),
            indent: -1,
            indents: Vec::new(),
            flow_level: 0,
            tokens_parsed: 0,
            leading_whitespace: true,
            flow_mapping_started: false,
            implicit_flow_mapping_states: Vec::new(),
        }
    }

    // ---------------------------------------------------------------- cursor

    fn peek(&self) -> u8 {
        self.src.get(self.pos).copied().unwrap_or(0)
    }

    fn peek_nth(&self, n: usize) -> u8 {
        self.src.get(self.pos + n).copied().unwrap_or(0)
    }

    fn is_eof(&self) -> bool {
        self.pos >= self.src.len()
    }

    /// End of input, or a NUL byte (libyaml's EOF sentinel).
    fn next_is_z(&self) -> bool {
        self.peek() == 0
    }

    /// Consume one character (possibly multi-byte). Must not be a line break.
    fn bump(&mut self) {
        debug_assert!(!self.is_eof());
        let b = self.src[self.pos];
        // 0 leading ones = ASCII (width 1); otherwise the count is the width.
        let width = (b.leading_ones() as usize).max(1);
        self.pos = (self.pos + width).min(self.src.len());
        self.col += 1;
        self.leading_whitespace = false;
    }

    /// Consume one blank character (space or tab) without clearing
    /// `leading_whitespace`.
    fn bump_blank(&mut self) {
        debug_assert!(is_blank(self.peek()));
        self.pos += 1;
        self.col += 1;
    }

    /// Consume a line break (`\n`, `\r` or `\r\n`).
    fn bump_break(&mut self) {
        debug_assert!(is_break(self.peek()));
        if self.peek() == b'\r' && self.peek_nth(1) == b'\n' {
            self.pos += 1;
        }
        self.pos += 1;
        self.col = 0;
        self.line += 1;
        self.leading_whitespace = true;
    }

    /// Consume a line break; crossing a line break in block context re-allows
    /// a simple key.
    fn bump_break_in_stream(&mut self) {
        self.bump_break();
        if self.flow_level == 0 {
            self.simple_key_allowed = true;
        }
    }

    /// Consume a BOM without affecting the column.
    fn bump_bom(&mut self) {
        debug_assert!(self.next_is_bom());
        self.pos += BOM.len();
    }

    /// Bulk-consume bytes while `keep` holds. To never split a multi-byte
    /// character, the predicate must treat all bytes >= 0x80 uniformly (keep
    /// all of them, or reject all of them — a rejected lead byte stops the run
    /// at a character boundary).
    #[inline]
    fn bump_while(&mut self, keep: impl Fn(u8) -> bool) {
        let start = self.pos;
        let mut i = start;
        while i < self.src.len() && keep(self.src[i]) {
            i += 1;
        }
        if i > start {
            self.col += char_count(&self.src[start..i]);
            self.pos = i;
            self.leading_whitespace = false;
        }
    }

    /// Bulk-consume a run of spaces without clearing `leading_whitespace`.
    /// Returns the number of spaces consumed.
    #[inline]
    fn bump_space_run(&mut self) -> usize {
        let start = self.pos;
        while self.pos < self.src.len() && self.src[self.pos] == b' ' {
            self.pos += 1;
        }
        let n = self.pos - start;
        self.col += n;
        n
    }

    fn next_is_bom(&self) -> bool {
        self.src[self.pos..].starts_with(BOM)
    }

    fn next_is_document_start(&self) -> bool {
        self.src[self.pos..].starts_with(b"---") && is_blank_or_breakz(self.peek_nth(3))
    }

    fn next_is_document_end(&self) -> bool {
        self.src[self.pos..].starts_with(b"...") && is_blank_or_breakz(self.peek_nth(3))
    }

    fn next_is_document_indicator(&self) -> bool {
        self.next_is_document_start() || self.next_is_document_end()
    }

    /// Whether the next characters may be part of a plain scalar.
    /// See 7.3.3. Plain Style. This is context-sensitive: flow indicators only
    /// terminate a plain scalar inside a flow collection.
    fn next_can_be_plain_scalar(&self, in_flow: bool) -> bool {
        !ends_plain_scalar(self.peek(), self.peek_nth(1), in_flow)
    }

    fn error(&self, kind: ErrorKind, at: usize) -> Error {
        let end = (at + 1).min(self.src.len()).max(at);
        Error::new(kind, span(at, end))
    }

    // -------------------------------------------------------------- whitespace

    /// Record a `#` comment (cursor must be at the `#`) as trivia and consume
    /// it up to (excluding) the line break.
    fn eat_comment(&mut self) {
        debug_assert!(self.peek() == b'#');
        let start = self.pos;
        self.bump_while(|b| !is_breakz(b));
        self.comments.push(Comment { span: span(start, self.pos) });
    }

    /// Skip over whitespace, comments and line breaks until the next token.
    fn skip_to_next_token(&mut self) -> ScanResult {
        loop {
            match self.peek() {
                // Tabs may not be used as indentation. "Indentation" only
                // exists as long as a block is started, but does not exist
                // inside flow-style constructs.
                b'\t'
                    if self.is_within_block()
                        && self.leading_whitespace
                        && self.col.cast_signed() < self.indent =>
                {
                    self.skip_ws_to_eol(true)?;
                    // If we have content on that line with a tab, error out.
                    if !is_breakz(self.peek()) {
                        return Err(self.error(ErrorKind::TabAsIndent, self.pos));
                    }
                }
                b' ' => {
                    self.bump_space_run();
                }
                b'\t' => self.bump_blank(),
                b'\n' | b'\r' => self.bump_break_in_stream(),
                b'#' => self.eat_comment(),
                0xEF if self.next_is_bom() => self.bump_bom(),
                _ => break,
            }
        }
        Ok(())
    }

    /// Skip spaces (and optionally tabs) and comments up to the end of line.
    /// Returns `(found_tabs, has_yaml_ws)`.
    fn skip_ws_to_eol(&mut self, skip_tabs: bool) -> ScanResult<(bool, bool)> {
        let mut found_tabs = false;
        let mut has_yaml_ws = false;
        loop {
            match self.peek() {
                b' ' => {
                    has_yaml_ws = true;
                    self.bump_space_run();
                }
                b'\t' if skip_tabs => {
                    found_tabs = true;
                    self.bump_blank();
                }
                // YAML comments must be preceded by whitespace.
                b'#' if !found_tabs && !has_yaml_ws && self.col != 0 => {
                    return Err(self.error(ErrorKind::UnexpectedToken("comment"), self.pos));
                }
                b'#' => self.eat_comment(),
                _ => break,
            }
        }
        Ok((found_tabs, has_yaml_ws))
    }

    /// Skip YAML whitespace (` `, line breaks) and comments. Errors if none found.
    fn skip_yaml_whitespace(&mut self) -> ScanResult {
        let mut need_whitespace = true;
        loop {
            match self.peek() {
                b' ' => {
                    self.bump_blank();
                    need_whitespace = false;
                }
                b'\n' | b'\r' => {
                    self.bump_break_in_stream();
                    need_whitespace = false;
                }
                b'#' => self.eat_comment(),
                _ => break,
            }
        }
        if need_whitespace {
            Err(self.error(ErrorKind::UnexpectedToken("non-whitespace"), self.pos))
        } else {
            Ok(())
        }
    }

    // ------------------------------------------------------------- public API

    /// Return the next token in the stream, or `None` after `StreamEnd`.
    pub fn next_token(&mut self) -> ScanResult<Option<Token>> {
        if self.stream_end_produced {
            return Ok(None);
        }

        self.fetch_more_tokens()?;
        let Some(t) = self.tokens.pop_front() else {
            return Err(self.error(ErrorKind::UnexpectedEof, self.pos));
        };
        self.tokens_parsed += 1;

        if t.kind == TokenKind::StreamEnd {
            self.stream_end_produced = true;
        }
        Ok(Some(t))
    }

    fn fetch_more_tokens(&mut self) -> ScanResult {
        loop {
            let mut need_more = self.tokens.is_empty();
            if !need_more {
                self.stale_simple_keys()?;
                // If our next token to be emitted may be a key, fetch more context.
                need_more = self
                    .simple_keys
                    .iter()
                    .any(|sk| sk.possible && sk.token_number == self.tokens_parsed);
            }
            if !need_more {
                break;
            }
            self.fetch_next_token()?;
        }
        Ok(())
    }

    fn fetch_next_token(&mut self) -> ScanResult {
        if !self.stream_start_produced {
            self.fetch_stream_start();
            return Ok(());
        }
        self.skip_to_next_token()?;
        self.stale_simple_keys()?;

        self.unroll_indent(self.col.cast_signed());

        if self.next_is_z() {
            self.fetch_stream_end()?;
            return Ok(());
        }

        if self.col == 0 {
            if self.peek() == b'%' {
                return self.fetch_directive();
            } else if self.next_is_document_start() {
                return self.fetch_document_indicator(TokenKind::DocumentStart);
            } else if self.next_is_document_end() {
                self.fetch_document_indicator(TokenKind::DocumentEnd)?;
                self.skip_ws_to_eol(true)?;
                if !is_breakz(self.peek()) {
                    return Err(self.error(ErrorKind::ExpectedDocumentEnd, self.pos));
                }
                return Ok(());
            }
        }

        if self.col.cast_signed() < self.indent {
            return Err(self.error(ErrorKind::UnexpectedIndicator, self.pos));
        }

        let c = self.peek();
        let nc = self.peek_nth(1);
        match c {
            b'[' => self.fetch_flow_collection_start(TokenKind::FlowSequenceStart),
            b'{' => self.fetch_flow_collection_start(TokenKind::FlowMappingStart),
            b']' => self.fetch_flow_collection_end(TokenKind::FlowSequenceEnd),
            b'}' => self.fetch_flow_collection_end(TokenKind::FlowMappingEnd),
            b',' => self.fetch_flow_entry(),
            b'-' if is_blank_or_breakz(nc) => self.fetch_block_entry(),
            b'?' if is_blank_or_breakz(nc) => self.fetch_key(),
            b':' if is_blank_or_breakz(nc) => self.fetch_value(),
            b':' if self.flow_level > 0
                && (is_flow(nc) || self.pos == self.adjacent_value_allowed_at) =>
            {
                self.fetch_flow_value()
            }
            b'*' => self.fetch_anchor(true),
            b'&' => self.fetch_anchor(false),
            b'!' => self.fetch_tag(),
            b'|' if self.flow_level == 0 => self.fetch_block_scalar(true),
            b'>' if self.flow_level == 0 => self.fetch_block_scalar(false),
            b'\'' => self.fetch_flow_scalar(true),
            b'"' => self.fetch_flow_scalar(false),
            b'-' if !is_blank_or_breakz(nc) => self.fetch_plain_scalar(),
            b':' | b'?' if !is_blank_or_breakz(nc) && self.flow_level == 0 => {
                self.fetch_plain_scalar()
            }
            b'%' | b'@' | b'`' => Err(self.error(ErrorKind::InvalidChar, self.pos)),
            _ => self.fetch_plain_scalar(),
        }
    }

    /// Mark simple keys that can no longer be keys as such.
    fn stale_simple_keys(&mut self) -> ScanResult {
        // Only block-context simple keys can go stale.
        if self.flow_level > 0 {
            return Ok(());
        }
        for sk in &mut self.simple_keys {
            if sk.possible
                // If not in a flow construct, simple keys cannot span multiple lines.
                && (sk.line < self.line || sk.pos + 1024 < self.pos)
            {
                if sk.required {
                    return Err(Error::point(ErrorKind::InvalidSimpleKey, sk.pos));
                }
                sk.possible = false;
            }
        }
        Ok(())
    }

    // ----------------------------------------------------------------- fetches

    fn fetch_stream_start(&mut self) {
        self.indent = -1;
        self.stream_start_produced = true;
        self.simple_key_allowed = true;
        self.tokens.push_back(Token::synthesized(TokenKind::StreamStart, 0));
        self.simple_keys.push(SimpleKey::new());
    }

    fn fetch_stream_end(&mut self) -> ScanResult {
        // If the stream ended, we won't have more context. Stale all simple
        // keys; a required one is an error.
        for sk in &mut self.simple_keys {
            if sk.required && sk.possible {
                return Err(Error::point(ErrorKind::InvalidSimpleKey, sk.pos));
            }
            sk.possible = false;
        }

        self.unroll_indent(-1);
        self.remove_simple_key()?;
        self.simple_key_allowed = false;

        self.tokens.push_back(Token::synthesized(TokenKind::StreamEnd, self.pos));
        Ok(())
    }

    /// Scan a directive line. Directives are uninterpreted: any `%NAME ...` up
    /// to the end of the line is accepted (Prettier treats unknown directives
    /// and unsupported `%YAML` versions as warnings, i.e. accepts them).
    fn fetch_directive(&mut self) -> ScanResult {
        self.unroll_indent(-1);
        self.remove_simple_key()?;
        self.simple_key_allowed = false;

        let start = self.pos;
        // Consume up to the line break, stopping before a trailing comment
        // (a comment must be preceded by whitespace to count as one).
        let mut end = self.pos;
        loop {
            let b = self.peek();
            if is_breakz(b) {
                break;
            }
            if b == b'#' && self.pos > start && is_blank(self.src[self.pos - 1]) {
                self.eat_comment();
                break;
            }
            if is_blank(b) {
                self.bump_blank();
            } else {
                // A `#` NOT preceded by whitespace is part of the directive
                // content, so the run only stops at whitespace; the comment
                // check at the loop top handles blank-preceded `#`.
                self.bump_while(|b| !is_blank_or_breakz(b));
                end = self.pos;
            }
        }
        self.tokens.push_back(Token::new(TokenKind::Directive, span(start, end)));
        if is_break(self.peek()) {
            self.bump_break();
        }
        Ok(())
    }

    fn fetch_document_indicator(&mut self, kind: TokenKind) -> ScanResult {
        self.unroll_indent(-1);
        self.remove_simple_key()?;
        self.simple_key_allowed = false;

        let start = self.pos;
        self.pos += 3;
        self.col += 3;
        self.leading_whitespace = false;

        self.tokens.push_back(Token::new(kind, span(start, self.pos)));
        Ok(())
    }

    fn fetch_flow_collection_start(&mut self, kind: TokenKind) -> ScanResult {
        // The indicators '[' and '{' may start a simple key.
        self.save_simple_key();

        self.roll_one_col_indent();
        self.increase_flow_level()?;

        self.simple_key_allowed = true;

        let start = self.pos;
        self.bump();

        if kind == TokenKind::FlowMappingStart {
            self.flow_mapping_started = true;
        } else {
            self.implicit_flow_mapping_states.push(ImplicitMappingState::Possible);
        }

        let token_end = self.pos;
        self.skip_ws_to_eol(true)?;

        self.tokens.push_back(Token::new(kind, span(start, token_end)));
        Ok(())
    }

    fn fetch_flow_collection_end(&mut self, kind: TokenKind) -> ScanResult {
        self.remove_simple_key()?;
        self.decrease_flow_level();

        self.simple_key_allowed = false;

        if kind == TokenKind::FlowSequenceEnd {
            self.end_implicit_mapping(self.pos);
            self.implicit_flow_mapping_states.pop();
        }

        let start = self.pos;
        self.bump();
        let token_end = self.pos;
        self.skip_ws_to_eol(true)?;

        // A flow collection within a flow mapping can be a key. In that case,
        // the value may be adjacent to the `:`. Like `fetch_flow_scalar`, the
        // `:` may also be separated by comments and line breaks
        // (`{["key"] # c` + newline + `:value}` is still one pair).
        if self.flow_level > 0 {
            self.skip_to_next_token()?;
            self.adjacent_value_allowed_at = self.pos;
        }

        self.tokens.push_back(Token::new(kind, span(start, token_end)));
        Ok(())
    }

    fn fetch_flow_entry(&mut self) -> ScanResult {
        if self.flow_level == 0 {
            return Err(self.error(ErrorKind::UnexpectedFlowIndicator, self.pos));
        }
        self.remove_simple_key()?;
        self.simple_key_allowed = true;

        self.end_implicit_mapping(self.pos);

        let start = self.pos;
        self.bump();
        let token_end = self.pos;
        self.skip_ws_to_eol(true)?;

        self.tokens.push_back(Token::new(TokenKind::FlowEntry, span(start, token_end)));
        Ok(())
    }

    fn fetch_block_entry(&mut self) -> ScanResult {
        if self.flow_level > 0 {
            return Err(self.error(ErrorKind::UnexpectedIndicator, self.pos));
        }
        if !self.simple_key_allowed {
            return Err(self.error(ErrorKind::UnexpectedIndicator, self.pos));
        }

        // An anchor or tag at column 0 cannot apply to a sequence starting at
        // column 0 of the next line (yaml-test-suite G9HC).
        if let Some(Token { span, kind: TokenKind::Anchor | TokenKind::Tag, .. }) =
            self.tokens.back()
            && self.col == 0
            && self.indent > -1
            && span_starts_at_col0(self.src, *span)
        {
            return Err(Error::new(ErrorKind::UnexpectedIndicator, *span));
        }

        let start = self.pos;
        let col = self.col;
        self.bump();

        // Generate BLOCK-SEQUENCE-START if indented.
        self.roll_indent(col, None, TokenKind::BlockSequenceStart, start);
        let (found_tabs, _) = self.skip_ws_to_eol(true)?;
        if found_tabs && self.peek() == b'-' && is_blank_or_breakz(self.peek_nth(1)) {
            return Err(self.error(ErrorKind::TabAsIndent, self.pos));
        }

        self.skip_ws_to_eol(false)?;
        if is_break(self.peek()) || is_flow(self.peek()) {
            self.roll_one_col_indent();
        }

        self.remove_simple_key()?;
        self.simple_key_allowed = true;

        self.tokens.push_back(Token::new(TokenKind::BlockEntry, span(start, start + 1)));
        Ok(())
    }

    fn fetch_key(&mut self) -> ScanResult {
        let start = self.pos;
        if self.flow_level == 0 {
            // Check if we are allowed to start a new key (not necessarily simple).
            if !self.simple_key_allowed {
                return Err(self.error(ErrorKind::UnexpectedIndicator, self.pos));
            }
            self.roll_indent(self.col, None, TokenKind::BlockMappingStart, start);
        } else {
            // The scanner, upon emitting a `Key`, will prepend a `MappingStart` event.
            self.flow_mapping_started = true;
        }

        self.remove_simple_key()?;

        self.simple_key_allowed = self.flow_level == 0;

        self.bump();
        let token_end = self.pos;
        self.skip_yaml_whitespace()?;
        if self.peek() == b'\t' {
            return Err(self.error(ErrorKind::TabAsIndent, self.pos));
        }
        self.tokens.push_back(Token::new(TokenKind::Key, span(start, token_end)));
        Ok(())
    }

    /// Fetch a value in a mapping inside a flow collection, reached through a
    /// `:` NOT followed by a blank.
    fn fetch_flow_value(&mut self) -> ScanResult {
        let nc = self.peek_nth(1);
        // `["a":[]]` is valid (adjacent value after JSON-like key) while
        // `[a:[]]` is not (`[a: []]` is; `[a:b]` is the scalar `a:b`).
        if self.pos != self.adjacent_value_allowed_at && (nc == b'[' || nc == b'{') {
            return Err(self.error(ErrorKind::UnexpectedValue, self.pos));
        }
        self.fetch_value()
    }

    /// Fetch the `Value` token (after a `:`).
    fn fetch_value(&mut self) -> ScanResult {
        let sk = *self.simple_keys.last().unwrap();
        let start = self.pos;
        let start_col = self.col;
        let is_implicit_flow_mapping =
            !self.implicit_flow_mapping_states.is_empty() && !self.flow_mapping_started;
        if is_implicit_flow_mapping {
            *self.implicit_flow_mapping_states.last_mut().unwrap() = ImplicitMappingState::Inside;
        }

        // Skip over ':'.
        self.bump();
        if self.peek() == b'\t' {
            let (_, has_ws) = self.skip_ws_to_eol(true)?;
            if !has_ws && (self.peek() == b'-' || is_alpha(self.peek())) {
                return Err(self.error(ErrorKind::TabAsIndent, self.pos));
            }
        }

        if sk.possible {
            // Insert the simple key token.
            let tok = Token::synthesized(TokenKind::Key, sk.pos);
            self.insert_token(sk.token_number - self.tokens_parsed, tok);
            if is_implicit_flow_mapping {
                if sk.line < self.line {
                    return Err(self.error(ErrorKind::UnexpectedValue, start));
                }
                self.insert_token(
                    sk.token_number - self.tokens_parsed,
                    Token::synthesized(TokenKind::FlowMappingStart, sk.pos),
                );
            }

            // Add the BLOCK-MAPPING-START token if needed.
            self.roll_indent(sk.col, Some(sk.token_number), TokenKind::BlockMappingStart, sk.pos);
            self.roll_one_col_indent();

            self.simple_keys.last_mut().unwrap().possible = false;
            self.simple_key_allowed = false;
        } else {
            if is_implicit_flow_mapping {
                self.tokens.push_back(Token::synthesized(TokenKind::FlowMappingStart, start));
            }
            // The ':' indicator follows a complex key.
            if self.flow_level == 0 {
                if !self.simple_key_allowed {
                    return Err(self.error(ErrorKind::UnexpectedValue, start));
                }
                self.roll_indent(start_col, None, TokenKind::BlockMappingStart, start);
            }
            self.roll_one_col_indent();

            self.simple_key_allowed = self.flow_level == 0;
        }
        self.tokens.push_back(Token::new(TokenKind::Value, span(start, start + 1)));
        Ok(())
    }

    fn fetch_anchor(&mut self, alias: bool) -> ScanResult {
        self.save_simple_key();
        self.simple_key_allowed = false;

        let start = self.pos;
        self.bump(); // `&` or `*`
        self.bump_while(is_anchor_char);
        if self.pos == start + 1 {
            return Err(self.error(ErrorKind::EmptyAnchorName, start));
        }
        let kind = if alias { TokenKind::Alias } else { TokenKind::Anchor };
        self.tokens.push_back(Token::new(kind, span(start, self.pos)));
        Ok(())
    }

    fn fetch_tag(&mut self) -> ScanResult {
        self.save_simple_key();
        self.simple_key_allowed = false;

        let tok = self.scan_tag()?;
        self.tokens.push_back(tok);
        Ok(())
    }

    fn scan_tag(&mut self) -> ScanResult<Token> {
        let start = self.pos;

        if self.peek_nth(1) == b'<' {
            // Verbatim tag `!<...>`.
            self.bump();
            self.bump();
            self.bump_while(is_uri_char);
            if self.peek() != b'>' {
                return Err(self.error(ErrorKind::InvalidTag, start));
            }
            self.bump();
        } else {
            // `!`, `!suffix`, `!!suffix` or `!handle!suffix`.
            self.bump(); // leading `!`
            self.bump_while(is_alpha);
            if self.peek() == b'!' {
                // It was a handle; scan the suffix.
                self.bump();
            }
            self.bump_while(is_tag_char);
        }

        if is_blank_or_breakz(self.peek()) || (self.flow_level > 0 && is_flow(self.peek())) {
            Ok(Token::new(TokenKind::Tag, span(start, self.pos)))
        } else {
            Err(self.error(ErrorKind::InvalidTag, start))
        }
    }

    fn fetch_block_scalar(&mut self, literal: bool) -> ScanResult {
        self.save_simple_key();
        self.simple_key_allowed = true;
        let tok = self.scan_block_scalar(literal)?;
        self.tokens.push_back(tok);
        Ok(())
    }

    fn scan_block_scalar(&mut self, literal: bool) -> ScanResult<Token> {
        let start = self.pos;
        let mut chomping = Chomping::Clip;
        let mut increment: usize = 0;
        let style = if literal { ScalarStyle::Literal } else { ScalarStyle::Folded };

        // Skip `|` or `>`.
        self.bump();
        self.unroll_non_block_indents();

        // Parse the header: chomping and indentation indicators, in either order.
        if self.peek() == b'+' || self.peek() == b'-' {
            chomping = self.read_chomping();
            if self.peek().is_ascii_digit() {
                increment = self.read_block_indent(start)?;
            }
        } else if self.peek().is_ascii_digit() {
            increment = self.read_block_indent(start)?;
            if self.peek() == b'+' || self.peek() == b'-' {
                chomping = self.read_chomping();
            }
        }

        self.skip_ws_to_eol(true)?;

        if !is_breakz(self.peek()) {
            return Err(self.error(ErrorKind::InvalidBlockScalarHeader, start));
        }

        if is_break(self.peek()) {
            self.bump_break();
        }
        let content_start = self.pos;

        if self.peek() == b'\t' {
            return Err(self.error(ErrorKind::TabAsIndent, self.pos));
        }

        let mut indent: usize = 0;
        if increment > 0 {
            indent =
                if self.indent >= 0 { self.indent.cast_unsigned() + increment } else { increment };
        }

        // Scan the leading line breaks and determine the indentation level if needed.
        if indent == 0 {
            self.skip_block_scalar_first_line_indent(&mut indent);
        } else {
            self.skip_block_scalar_indent(indent);
        }

        let header_index = BlockHeaderIndex::new(self.block_headers.len());
        self.block_headers.push(BlockScalarHeader {
            chomping,
            indent: if increment > 0 { Some(increment as u32) } else { None },
            content_start: content_start as u32,
            // Placeholder for the no-content early return below;
            // the content loop overwrites it with the real text end.
            content_end: content_start as u32,
        });

        // End-of-stream with no content, e.g. `- |+`.
        if self.next_is_z() {
            return Ok(Token::new(
                TokenKind::Scalar(style, Some(header_index)),
                span(start, self.pos),
            ));
        }

        if self.col < indent && self.col.cast_signed() > self.indent {
            return Err(self.error(ErrorKind::InvalidBlockScalarIndent, self.pos));
        }

        // The scan may overshoot into the terminating line's indentation; the
        // token must end just after the last line break that belongs to the
        // scalar (its trailing breaks ARE content, a partial next indent is not).
        let mut content_end = self.pos;
        // Offset right after the last content character
        // (the loop only enters at lines that carry content, so every iteration advances it).
        let mut text_end = content_start;
        while self.col == indent && !(self.next_is_z()) {
            if indent == 0 && self.next_is_document_indicator() {
                break;
            }

            // Consume the content line, in bulk.
            self.bump_while(|b| !is_breakz(b));
            text_end = self.pos;

            if self.next_is_z() {
                content_end = self.pos;
                break;
            }
            self.bump_break();
            content_end = self.pos;

            // Eat the following indentation spaces and line breaks.
            if let Some(last_break_end) = self.skip_block_scalar_indent(indent) {
                content_end = last_break_end;
            }
        }
        self.block_headers[header_index.get()].content_end = text_end as u32;

        Ok(Token::new(TokenKind::Scalar(style, Some(header_index)), span(start, content_end)))
    }

    /// Read a block scalar chomping indicator: `+` (keep) or `-` (strip). The
    /// cursor must be at a `+` or `-`.
    fn read_chomping(&mut self) -> Chomping {
        debug_assert!(self.peek() == b'+' || self.peek() == b'-');
        let chomping = if self.peek() == b'+' { Chomping::Keep } else { Chomping::Strip };
        self.bump();
        chomping
    }

    /// Read a block scalar indentation indicator digit (`1`..=`9`) and return
    /// it. The cursor must be at an ASCII digit; `0` is not a valid indicator.
    fn read_block_indent(&mut self, start: usize) -> ScanResult<usize> {
        debug_assert!(self.peek().is_ascii_digit());
        if self.peek() == b'0' {
            return Err(self.error(ErrorKind::InvalidBlockScalarHeader, start));
        }
        let increment = (self.peek() - b'0') as usize;
        self.bump();
        Ok(increment)
    }

    /// Skip the block scalar indentation and empty lines.
    /// Returns the position just after the last consumed line break (i.e. the
    /// end of the scalar's content if the scan stops here), or `None` if no
    /// break was consumed.
    fn skip_block_scalar_indent(&mut self, indent: usize) -> Option<usize> {
        let mut last_break_end = None;
        loop {
            // Bounded bulk run of indentation spaces.
            while self.col < indent && self.peek() == b' ' {
                let want = indent - self.col;
                let mut i = self.pos;
                let limit = (self.pos + want).min(self.src.len());
                while i < limit && self.src[i] == b' ' {
                    i += 1;
                }
                self.col += i - self.pos;
                self.pos = i;
            }
            if is_break(self.peek()) {
                self.bump_break();
                last_break_end = Some(self.pos);
            } else {
                break;
            }
        }
        last_break_end
    }

    /// Determine the indentation level for a block scalar from the first line
    /// of its contents.
    fn skip_block_scalar_first_line_indent(&mut self, indent: &mut usize) {
        let mut max_indent = 0;
        loop {
            self.bump_space_run();
            if self.col > max_indent {
                max_indent = self.col;
            }
            if is_break(self.peek()) {
                self.bump_break();
            } else {
                break;
            }
        }

        *indent = max_indent.max((self.indent + 1).cast_unsigned());
        if self.indent > 0 {
            *indent = (*indent).max(1);
        }
    }

    fn fetch_flow_scalar(&mut self, single: bool) -> ScanResult {
        self.save_simple_key();
        self.simple_key_allowed = false;

        let tok = self.scan_flow_scalar(single)?;

        // To ensure JSON compatibility, if a key inside a flow mapping is
        // JSON-like, YAML allows the following value to be adjacent to the `:`.
        self.skip_to_next_token()?;
        self.adjacent_value_allowed_at = self.pos;

        self.tokens.push_back(tok);
        Ok(())
    }

    fn scan_flow_scalar(&mut self, single: bool) -> ScanResult<Token> {
        let start = self.pos;
        let start_line = self.line;

        // Eat the left quote.
        self.bump();

        loop {
            if self.col == 0 && self.next_is_document_indicator() {
                return Err(self.error(ErrorKind::UnterminatedFlowScalar, start));
            }
            if self.next_is_z() {
                return Err(self.error(ErrorKind::UnterminatedFlowScalar, start));
            }
            if self.col.cast_signed() < self.indent {
                return Err(self.error(ErrorKind::UnterminatedFlowScalar, start));
            }

            // Consume non-whitespace characters, in bulk up to the next
            // special character (quote / escape / whitespace).
            let mut done = false;
            while !is_blank_or_breakz(self.peek()) {
                if single {
                    self.bump_while(|b| !is_blank_or_breakz(b) && b != b'\'');
                } else {
                    self.bump_while(|b| !is_blank_or_breakz(b) && b != b'"' && b != b'\\');
                }
                match self.peek() {
                    b'\'' if single && self.peek_nth(1) == b'\'' => {
                        self.bump();
                        self.bump();
                    }
                    b'\'' if single => {
                        done = true;
                        break;
                    }
                    b'"' if !single => {
                        done = true;
                        break;
                    }
                    b'\\' if !single && is_break(self.peek_nth(1)) => {
                        self.bump();
                        self.bump_break();
                        break;
                    }
                    b'\\' if !single => {
                        self.scan_flow_scalar_escape(start)?;
                    }
                    _ => break,
                }
            }
            if done {
                break;
            }

            // Consume blank characters.
            loop {
                match self.peek() {
                    b' ' => {
                        self.bump_space_run();
                    }
                    b'\t' => {
                        if self.leading_whitespace && self.col.cast_signed() < self.indent {
                            return Err(self.error(ErrorKind::TabAsIndent, self.pos));
                        }
                        self.bump_blank();
                    }
                    b'\n' | b'\r' => self.bump_break(),
                    _ => break,
                }
            }
        }

        // Eat the right quote.
        self.bump();
        let token_end = self.pos;
        // Ensure there is no invalid trailing content.
        self.skip_ws_to_eol(true)?;
        match self.peek() {
            // These can be encountered in flow sequences or mappings.
            b',' | b'}' | b']' if self.flow_level > 0 => {}
            // An end-of-line / end-of-stream is fine. No trailing content.
            c if is_breakz(c) => {}
            // `:` can be encountered if our scalar is a key. Outside of flow
            // contexts, keys cannot span multiple lines.
            b':' if self.flow_level > 0 || self.line == start_line => {}
            _ => {
                return Err(
                    self.error(ErrorKind::UnexpectedToken("content after quoted scalar"), self.pos)
                );
            }
        }

        let style = if single { ScalarStyle::SingleQuoted } else { ScalarStyle::DoubleQuoted };
        Ok(Token::new(TokenKind::Scalar(style, None), span(start, token_end)))
    }

    /// Validate an escape sequence in a double quoted scalar. The cursor must
    /// be at the `\`.
    fn scan_flow_scalar_escape(&mut self, start: usize) -> ScanResult {
        let code_length = match self.peek_nth(1) {
            b'0' | b'a' | b'b' | b't' | b'\t' | b'n' | b'v' | b'f' | b'r' | b'e' | b' ' | b'"'
            | b'/' | b'\\' | b'N' | b'_' | b'L' | b'P' => 0,
            b'x' => 2,
            b'u' => 4,
            b'U' => 8,
            _ => {
                return Err(self.error(ErrorKind::UnexpectedToken("escape character"), start));
            }
        };
        self.bump();
        self.bump();

        if code_length > 0 {
            let mut value: u32 = 0;
            for i in 0..code_length {
                let c = self.peek_nth(i);
                if !c.is_ascii_hexdigit() {
                    return Err(self.error(ErrorKind::UnexpectedToken("hexadecimal digit"), start));
                }
                let digit = match c {
                    b'0'..=b'9' => c - b'0',
                    b'a'..=b'f' => c - b'a' + 10,
                    _ => c - b'A' + 10,
                };
                value = (value << 4) + u32::from(digit);
            }
            if char::from_u32(value).is_none() {
                return Err(self.error(ErrorKind::InvalidChar, start));
            }
            // Hex digits are ASCII; advance in one step.
            self.pos += code_length;
            self.col += code_length;
        }
        Ok(())
    }

    fn fetch_plain_scalar(&mut self) -> ScanResult {
        self.save_simple_key();
        self.simple_key_allowed = false;

        let tok = self.scan_plain_scalar()?;
        self.tokens.push_back(tok);
        Ok(())
    }

    fn scan_plain_scalar(&mut self) -> ScanResult<Token> {
        self.unroll_non_block_indents();
        let indent = self.indent + 1;
        let start = self.pos;

        if self.flow_level > 0 && self.col.cast_signed() < indent {
            return Err(self.error(ErrorKind::UnexpectedIndicator, start));
        }

        let mut end = self.pos;
        let mut consumed_content = false;

        loop {
            if (self.leading_whitespace && self.next_is_document_indicator()) || self.peek() == b'#'
            {
                break;
            }

            if self.flow_level > 0 && self.peek() == b'-' && is_flow(self.peek_nth(1)) {
                return Err(self.error(ErrorKind::UnexpectedIndicator, self.pos));
            }

            if !is_blank_or_breakz(self.peek())
                && self.next_can_be_plain_scalar(self.flow_level > 0)
            {
                self.leading_whitespace = false;
                // Add content non-blank characters to the scalar, in bulk.
                let in_flow = self.flow_level > 0;
                let run_start = self.pos;
                let mut i = self.pos;
                while let Some(&b) = self.src.get(i) {
                    let next = self.src.get(i + 1).copied().unwrap_or(0);
                    if is_blank_or_breakz(b) || ends_plain_scalar(b, next, in_flow) {
                        break;
                    }
                    i += 1;
                }
                if i > run_start {
                    self.col += char_count(&self.src[run_start..i]);
                    self.pos = i;
                }
                end = self.pos;
                consumed_content = true;
            }

            // We may reach the end of a plain scalar if we reach EOF, `: ` or
            // a flow character in a flow context.
            if !(is_blank(self.peek()) || is_break(self.peek())) {
                break;
            }

            // Process blank characters.
            loop {
                match self.peek() {
                    b' ' => {
                        self.bump_space_run();
                    }
                    b'\t' if self.leading_whitespace && self.col.cast_signed() < indent => {
                        // Tabs in an indentation column are allowed if and
                        // only if the line is empty.
                        self.skip_ws_to_eol(true)?;
                        if !is_breakz(self.peek()) {
                            return Err(self.error(ErrorKind::TabAsIndent, self.pos));
                        }
                    }
                    b'\t' => self.bump_blank(),
                    b'\n' | b'\r' => self.bump_break(),
                    _ => break,
                }
            }

            // Check indentation level.
            if self.flow_level == 0 && self.col.cast_signed() < indent {
                break;
            }
        }

        if self.leading_whitespace {
            self.simple_key_allowed = true;
        }

        if consumed_content {
            Ok(Token::new(TokenKind::Scalar(ScalarStyle::Plain, None), span(start, end)))
        } else {
            // `fetch_plain_scalar` must absolutely consume at least one byte;
            // an empty plain scalar happens with erroneous inputs like `{...`.
            Err(self.error(ErrorKind::ExpectedNode, start))
        }
    }

    // ------------------------------------------------------- indent bookkeeping

    /// Add an indentation level to the stack with the given block token, if needed.
    fn roll_indent(&mut self, col: usize, number: Option<usize>, kind: TokenKind, at: usize) {
        if self.flow_level > 0 {
            return;
        }

        // If the last indent was a non-block indent, remove it. We prepared an
        // indent that we thought we wouldn't use, but realized just now that
        // it is a block indent.
        if self.indent <= col.cast_signed()
            && let Some(indent) = self.indents.pop_if(|indent| !indent.needs_block_end)
        {
            self.indent = indent.indent;
        }

        if self.indent < col.cast_signed() {
            self.indents.push(Indent { indent: self.indent, needs_block_end: true });
            self.indent = col.cast_signed();
            let tokens_parsed = self.tokens_parsed;
            match number {
                Some(n) => self.insert_token(n - tokens_parsed, Token::synthesized(kind, at)),
                None => self.tokens.push_back(Token::synthesized(kind, at)),
            }
        }
    }

    /// Pop indentation levels from the stack as much as needed.
    fn unroll_indent(&mut self, col: isize) {
        if self.flow_level > 0 {
            return;
        }
        while self.indent > col {
            let indent = self.indents.pop().unwrap();
            self.indent = indent.indent;
            if indent.needs_block_end {
                self.tokens.push_back(Token::synthesized(TokenKind::BlockEnd, self.pos));
            }
        }
    }

    /// Add an indentation level of 1 column that does not start a block.
    fn roll_one_col_indent(&mut self) {
        if self.flow_level == 0 && self.indents.last().is_some_and(|x| x.needs_block_end) {
            self.indents.push(Indent { indent: self.indent, needs_block_end: false });
            self.indent += 1;
        }
    }

    /// Unroll all indents created with [`Self::roll_one_col_indent`].
    fn unroll_non_block_indents(&mut self) {
        while let Some(indent) = self.indents.pop_if(|indent| !indent.needs_block_end) {
            self.indent = indent.indent;
        }
    }

    // --------------------------------------------------- simple key bookkeeping

    /// Mark the next token to be inserted as a potential simple key.
    fn save_simple_key(&mut self) {
        if self.simple_key_allowed {
            let required = self.flow_level == 0
                && self.indent == self.col.cast_signed()
                && self.indents.last().is_some_and(|i| i.needs_block_end);
            let sk = SimpleKey {
                possible: true,
                required,
                token_number: self.tokens_parsed + self.tokens.len(),
                pos: self.pos,
                line: self.line,
                col: self.col,
            };
            *self.simple_keys.last_mut().unwrap() = sk;
        }
    }

    fn remove_simple_key(&mut self) -> ScanResult {
        let last = self.simple_keys.last_mut().unwrap();
        if last.possible && last.required {
            return Err(Error::point(ErrorKind::InvalidSimpleKey, last.pos));
        }
        last.possible = false;
        Ok(())
    }

    // ----------------------------------------------------------- flow plumbing

    fn increase_flow_level(&mut self) -> ScanResult {
        self.simple_keys.push(SimpleKey::new());
        self.flow_level = self
            .flow_level
            .checked_add(1)
            .ok_or_else(|| self.error(ErrorKind::UnexpectedIndicator, self.pos))?;
        Ok(())
    }

    fn decrease_flow_level(&mut self) {
        if self.flow_level > 0 {
            self.flow_level -= 1;
            self.simple_keys.pop().unwrap();
        }
    }

    /// If an implicit mapping had started, end it (does not pop the state).
    fn end_implicit_mapping(&mut self, at: usize) {
        if let Some(implicit_mapping) = self.implicit_flow_mapping_states.last_mut()
            && *implicit_mapping == ImplicitMappingState::Inside
        {
            self.flow_mapping_started = false;
            *implicit_mapping = ImplicitMappingState::Possible;
            self.tokens.push_back(Token::synthesized(TokenKind::FlowMappingEnd, at));
        }
    }

    /// Return whether the scanner is inside a block but outside a flow sequence.
    fn is_within_block(&self) -> bool {
        !self.indents.is_empty()
    }

    /// Insert a token at the given position.
    fn insert_token(&mut self, pos: usize, tok: Token) {
        assert!(pos <= self.tokens.len());
        self.tokens.insert(pos, tok);
    }
}

/// Construct a [`Span`] from byte-offset cursor positions.
#[inline]
fn span(start: usize, end: usize) -> Span {
    Span::new(start as u32, end as u32)
}

/// The number of characters in a byte slice (counting non-continuation bytes).
#[inline]
fn char_count(bytes: &[u8]) -> usize {
    // `is_ascii` is vectorized in std; content runs are overwhelmingly ASCII.
    if bytes.is_ascii() {
        bytes.len()
    } else {
        bytes.iter().filter(|&&b| (b & 0xC0) != 0x80).count()
    }
}

/// Whether the given span starts at column 0 (its start is at the beginning of
/// the source or right after a line break).
fn span_starts_at_col0(src: &[u8], span: Span) -> bool {
    span.start == 0 || matches!(src.get(span.start as usize - 1), Some(b'\n' | b'\r'))
}
