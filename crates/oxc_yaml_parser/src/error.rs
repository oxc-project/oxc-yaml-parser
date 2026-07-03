//! Error management.

use crate::pos::Span;
use std::fmt::Display;

#[derive(Clone, Debug)]
pub struct Error {
    pub kind: ErrorKind,
    pub span: Span,
}

impl Error {
    pub(crate) fn new(kind: ErrorKind, span: Span) -> Self {
        Self { kind, span }
    }

    /// An error pointing at a single byte offset.
    #[expect(clippy::cast_possible_truncation)] // sources are bounded to u32
    pub(crate) fn point(kind: ErrorKind, at: usize) -> Self {
        Self { kind, span: Span::new(at as u32, at as u32 + 1) }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    /// The source exceeds the maximum supported size (4 GiB).
    SourceTooLong,
    UnexpectedEof,
    /// A control character or other char that cannot appear in a YAML stream.
    InvalidChar,
    /// Tabs used for indentation.
    TabAsIndent,
    /// Directives must be followed by a `---` document start marker.
    ExpectedDocumentStart,
    /// Content found after a document where a new document or EOF was expected.
    ExpectedDocumentEnd,
    /// `&` or `*` with an empty name.
    EmptyAnchorName,
    /// Malformed `!` tag property.
    InvalidTag,
    /// Malformed block scalar header (`|`/`>` + indicators).
    InvalidBlockScalarHeader,
    /// A block scalar's content line is indented less than its detected indentation.
    InvalidBlockScalarIndent,
    UnterminatedFlowScalar,
    /// `,` `[` `]` `{` `}` misplaced in a flow collection.
    UnexpectedFlowIndicator,
    /// A simple key is too long or spans multiple lines.
    InvalidSimpleKey,
    /// `:` or `-` or `?` in a position where a block collection cannot start.
    UnexpectedIndicator,
    /// Mapping value (`:`) in an invalid context.
    UnexpectedValue,
    /// A node was expected but something else was found.
    ExpectedNode,
    /// Two anchors or two tags on the same node, etc.
    DuplicatedNodeProperty,
    /// Generic "unexpected token" during parsing.
    UnexpectedToken(&'static str),
}

impl Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} at {}..{}", self.kind, self.span.start, self.span.end)
    }
}

impl Display for ErrorKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            ErrorKind::SourceTooLong => "source exceeds the maximum supported size (4 GiB)",
            ErrorKind::UnexpectedEof => "unexpected end of input",
            ErrorKind::InvalidChar => "invalid character",
            ErrorKind::TabAsIndent => "tabs are not allowed as indentation",
            ErrorKind::ExpectedDocumentStart => "expected document start marker `---`",
            ErrorKind::ExpectedDocumentEnd => "expected the document to end",
            ErrorKind::EmptyAnchorName => "anchor or alias name cannot be empty",
            ErrorKind::InvalidTag => "invalid tag property",
            ErrorKind::InvalidBlockScalarHeader => "invalid block scalar header",
            ErrorKind::InvalidBlockScalarIndent => "invalid block scalar indentation",
            ErrorKind::UnterminatedFlowScalar => "unterminated quoted scalar",
            ErrorKind::UnexpectedFlowIndicator => "unexpected flow collection indicator",
            ErrorKind::InvalidSimpleKey => "invalid simple key",
            ErrorKind::UnexpectedIndicator => "unexpected indicator",
            ErrorKind::UnexpectedValue => "mapping values are not allowed in this context",
            ErrorKind::ExpectedNode => "expected a node",
            ErrorKind::DuplicatedNodeProperty => "duplicated node property",
            ErrorKind::UnexpectedToken(what) => {
                return write!(f, "unexpected {what}");
            }
        };
        f.write_str(message)
    }
}

impl std::error::Error for Error {}
