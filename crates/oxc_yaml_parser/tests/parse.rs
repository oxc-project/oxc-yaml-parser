use oxc_yaml_parser::{Allocator, Parser, Span, ast::*};

fn parse<'a>(allocator: &'a Allocator, source: &'a str) -> Root<'a> {
    Parser::new(allocator, source).parse().unwrap()
}

/// The first document's body node.
fn body<'r, 'a>(root: &'r Root<'a>) -> &'r Node<'a> {
    root.children[0].body.content.as_ref().unwrap()
}

/// A mapping item's value node.
fn value_node<'r, 'a>(item: &'r MappingItem<'a>) -> &'r Node<'a> {
    item.value_content().unwrap()
}

#[test]
fn empty_stream() {
    let allocator = Allocator::default();
    let root = parse(&allocator, "");
    assert!(root.children.is_empty());
    assert!(root.comments.is_empty());
}

#[test]
fn comments_are_collected_with_spans() {
    let allocator = Allocator::default();
    let source = "# leading\nkey: value # trailing\n# end\n";
    let root = parse(&allocator, source);
    let texts: Vec<&str> = root.comments.iter().map(|c| c.span.slice(source)).collect();
    assert_eq!(texts, ["# leading", "# trailing", "# end"]);
}

#[test]
fn comments_carry_own_line_column() {
    let allocator = Allocator::default();
    let source = "# top\nkey: value # trailing\nmap:\n  a: 1\n  # indented\n\t# after tab\n";
    let root = parse(&allocator, source);
    let columns: Vec<Option<u32>> = root.comments.iter().map(|c| c.own_line_column).collect();
    // Own-line comments report the `#` column (a tab counts as one);
    // a comment trailing other content reports `None`.
    assert_eq!(columns, [Some(0), None, Some(2), Some(1)]);
}

#[test]
fn plain_scalar_span_excludes_trailing_whitespace() {
    let allocator = Allocator::default();
    let source = "key: value  \n";
    let root = parse(&allocator, source);
    let Content::Mapping(mapping) = &body(&root).content else {
        panic!("expected mapping");
    };
    let value = value_node(&mapping.children[0]);
    assert_eq!(value.span.slice(source), "value");
}

#[test]
fn multiline_plain_scalar_span() {
    let allocator = Allocator::default();
    let source = "key: one\n  two\n";
    let root = parse(&allocator, source);
    let Content::Mapping(mapping) = &body(&root).content else {
        panic!("expected mapping");
    };
    let value = value_node(&mapping.children[0]);
    assert_eq!(value.span.slice(source), "one\n  two");
}

#[test]
fn mapping_value_span_starts_at_colon() {
    let allocator = Allocator::default();
    let source = "key: value\nempty:\n";
    let root = parse(&allocator, source);
    let Content::Mapping(mapping) = &body(&root).content else {
        panic!("expected mapping");
    };
    let value = mapping.children[0].value.as_ref().unwrap();
    assert_eq!(value.span.slice(source), ": value");
    // `key:` with no value: the value node is just the `:`.
    let empty = mapping.children[1].value.as_ref().unwrap();
    assert!(empty.content.is_none());
    assert_eq!(empty.span.slice(source), ":");
}

#[test]
fn value_is_absent_without_colon() {
    // A lone key in a flow mapping has no `:` in the source: no value node.
    let allocator = Allocator::default();
    let source = "{a}\n";
    let root = parse(&allocator, source);
    let Content::FlowMapping(mapping) = &body(&root).content else {
        panic!("expected flow mapping");
    };
    let item = &mapping.children[0];
    assert!(item.key.is_some());
    assert!(item.value.is_none());
}

#[test]
fn key_is_absent_without_indicator_or_content() {
    // `: value` has neither a `?` nor key content: no key node.
    let allocator = Allocator::default();
    let source = ": value\n";
    let root = parse(&allocator, source);
    let Content::Mapping(mapping) = &body(&root).content else {
        panic!("expected mapping");
    };
    let item = &mapping.children[0];
    assert!(item.key.is_none());
    assert_eq!(item.value.as_ref().unwrap().span.slice(source), ": value");
}

#[test]
fn block_scalar_header() {
    let allocator = Allocator::default();
    let source = "key: |2+\n  text\n";
    let root = parse(&allocator, source);
    let Content::Mapping(mapping) = &body(&root).content else {
        panic!("expected mapping");
    };
    let Content::BlockLiteral(block) = &value_node(&mapping.children[0]).content else {
        panic!("expected block literal");
    };
    assert_eq!(block.chomping, Chomping::Keep);
    assert_eq!(block.indent, Some(2));
    assert_eq!(Span::new(block.content_start, block.span.end).slice(source), "  text\n");
    assert_eq!(Span::new(block.content_start, block.content_end).slice(source), "  text");
}

#[test]
fn block_scalar_span_excludes_next_entry_indent() {
    // The scan overshoots into the terminating line's indentation; the
    // token must end just after the scalar's last line break. Trailing
    // empty lines ARE content, the next entry's partial indent is not.
    let allocator = Allocator::default();
    let source = "a:\n  b: |\n    text\n\n  c: d\n";
    let root = parse(&allocator, source);
    let Content::Mapping(outer) = &body(&root).content else {
        panic!("expected mapping");
    };
    let Content::Mapping(inner) = &value_node(&outer.children[0]).content else {
        panic!("expected nested mapping");
    };
    assert_eq!(inner.children.len(), 2);
    let Content::BlockLiteral(block) = &value_node(&inner.children[0]).content else {
        panic!("expected block literal");
    };
    assert_eq!(Span::new(block.content_start, block.span.end).slice(source), "    text\n\n");
    // `content_end` stops right after the last content character.
    assert_eq!(Span::new(block.content_start, block.content_end).slice(source), "    text");
}

#[test]
fn contentless_block_scalar_span_excludes_next_entry_indent() {
    // Same rule with no content line at all:
    // the leading blank-line skip overshoots into the next entry's indentation, the span must not.
    let allocator = Allocator::default();
    let source = "example:\n  - |+\n\n  - .\n";
    let root = parse(&allocator, source);
    let Content::Mapping(mapping) = &body(&root).content else {
        panic!("expected mapping");
    };
    let Content::Sequence(sequence) = &value_node(&mapping.children[0]).content else {
        panic!("expected sequence");
    };
    let Content::BlockLiteral(block) = &sequence.children[0].content.as_ref().unwrap().content
    else {
        panic!("expected block literal");
    };
    assert_eq!(Span::new(block.content_start, block.span.end).slice(source), "\n");
    assert_eq!(block.content_start, block.content_end);
}

#[test]
fn contentless_block_scalar_span_keeps_eos_trailing_spaces() {
    // Unlike mid-stream (span ends at the scalar's last line break, above),
    // trailing spaces at end of stream stay in the span (see the scanner's end-of-stream return for why).
    let allocator = Allocator::default();
    let source = "key: |+\n\n  ";
    let root = parse(&allocator, source);
    let Content::Mapping(mapping) = &body(&root).content else {
        panic!("expected mapping");
    };
    let Content::BlockLiteral(block) = &value_node(&mapping.children[0]).content else {
        panic!("expected block literal");
    };
    assert_eq!(Span::new(block.content_start, block.span.end).slice(source), "\n  ");
    assert_eq!(block.content_start, block.content_end);
}

#[test]
fn block_scalar_content_range_holds_no_comments() {
    // The GUARANTEE documented on `BlockScalar`:
    // a `#` line indented to the content is value,
    // a lesser-indented one ends the scalar first,
    // and the trailing break run holds only breaks.
    // So the sole comment within `span` is the header line's trailing one,
    // ending before `content_start`.
    let allocator = Allocator::default();
    let source =
        "key: | # header\n  # value not comment\n\n# after\nnext: |+\n  text\n\n\nlast: 1\n";
    let root = parse(&allocator, source);

    let Content::Mapping(mapping) = &body(&root).content else {
        panic!("expected mapping");
    };
    let texts: Vec<&str> = root.comments.iter().map(|c| c.span.slice(source)).collect();
    assert_eq!(texts, ["# header", "# after"]);
    for item in &mapping.children {
        let Some(node) = item.value_content() else { continue };
        let (Content::BlockLiteral(block) | Content::BlockFolded(block)) = &node.content else {
            continue;
        };
        for comment in &root.comments {
            assert!(
                comment.span.end <= block.content_start || comment.span.start >= block.span.end,
                "comment {:?} overlaps block scalar content range",
                comment.span.slice(source),
            );
        }
    }
}

#[test]
fn block_scalar_without_content() {
    let allocator = Allocator::default();
    let source = "key: |+\n";
    let root = parse(&allocator, source);
    let Content::Mapping(mapping) = &body(&root).content else {
        panic!("expected mapping");
    };
    let Content::BlockLiteral(block) = &value_node(&mapping.children[0]).content else {
        panic!("expected block literal");
    };
    assert_eq!(block.content_start, block.content_end);
}

#[test]
fn anchor_tag_and_alias() {
    let allocator = Allocator::default();
    let source = "a: &x !!str hello\nb: *x\n";
    let root = parse(&allocator, source);
    let Content::Mapping(mapping) = &body(&root).content else {
        panic!("expected mapping");
    };
    let a = value_node(&mapping.children[0]);
    assert_eq!(a.props.anchor.unwrap().span.slice(source), "&x");
    assert_eq!(a.props.tag.unwrap().span.slice(source), "!!str");
    // The node span covers the props; the content span is the scalar alone.
    assert_eq!(a.span.slice(source), "&x !!str hello");
    assert_eq!(a.content.span().slice(source), "hello");
    let b = value_node(&mapping.children[1]);
    assert!(matches!(b.content, Content::Alias(_)));
}

#[test]
fn explicit_key() {
    let allocator = Allocator::default();
    let source = "? key\n: value\n";
    let root = parse(&allocator, source);
    let Content::Mapping(mapping) = &body(&root).content else {
        panic!("expected mapping");
    };
    let key = mapping.children[0].key.as_ref().unwrap();
    assert!(key.explicit);
    // An explicit key's span starts at the `?` indicator.
    assert_eq!(key.span.slice(source), "? key");
}

#[test]
fn explicit_key_without_content() {
    let allocator = Allocator::default();
    let source = "?\n: value\n";
    let root = parse(&allocator, source);
    let Content::Mapping(mapping) = &body(&root).content else {
        panic!("expected mapping");
    };
    let key = mapping.children[0].key.as_ref().unwrap();
    assert!(key.explicit);
    assert!(key.content.is_none());
    assert_eq!(key.span.slice(source), "?");
}

#[test]
fn multi_document_stream() {
    let allocator = Allocator::default();
    let source = "---\na\n...\n---\nb\n";
    let root = parse(&allocator, source);
    assert_eq!(root.children.len(), 2);
    assert!(root.children[0].directives_end_marker.is_some());
    assert!(root.children[0].document_end_marker.is_some());
    assert!(root.children[1].document_end_marker.is_none());
}

#[test]
fn directives_are_uninterpreted() {
    let allocator = Allocator::default();
    let source = "%YAML 1.3\n%FOO bar baz\n---\nx\n";
    let root = parse(&allocator, source);
    let directives = &root.children[0].head.directives;
    assert_eq!(directives.len(), 2);
    assert_eq!(directives[0].name, "YAML");
    assert_eq!(directives[0].parameters.as_slice(), ["1.3"]);
    assert_eq!(directives[1].name, "FOO");
    assert_eq!(directives[1].parameters.as_slice(), ["bar", "baz"]);
}

#[test]
fn indentless_sequence() {
    let allocator = Allocator::default();
    let source = "key:\n- a\n- b\n";
    let root = parse(&allocator, source);
    let Content::Mapping(mapping) = &body(&root).content else {
        panic!("expected mapping");
    };
    let Content::Sequence(seq) = &value_node(&mapping.children[0]).content else {
        panic!("expected sequence");
    };
    assert_eq!(seq.children.len(), 2);
}

#[test]
fn no_indentless_sequence_in_sequence_item_position() {
    // `- !!tag` followed by `- next` is an empty tagged node plus the
    // parent's next entry, not a nested indentless sequence.
    let allocator = Allocator::default();
    let source = "- !!tag\n- next\n";
    let root = parse(&allocator, source);
    let Content::Sequence(seq) = &body(&root).content else {
        panic!("expected sequence");
    };
    assert_eq!(seq.children.len(), 2);
    let first = seq.children[0].content.as_ref().unwrap();
    assert!(matches!(first.content, Content::Plain(_)));
    assert_eq!(first.props.tag.unwrap().span.slice(source), "!!tag");
    // The synthesized empty content sits right after the props; the node
    // span covers the props.
    assert!(first.content.span().is_empty());
    assert_eq!(first.span.slice(source), "!!tag");
}

#[test]
fn flow_pair_in_sequence() {
    let allocator = Allocator::default();
    let source = "[a: b, c]\n";
    let root = parse(&allocator, source);
    let Content::FlowSequence(seq) = &body(&root).content else {
        panic!("expected flow sequence");
    };
    assert_eq!(seq.children.len(), 2);
    assert!(matches!(seq.children[0], FlowSequenceEntry::Pair(_)));
    assert!(matches!(seq.children[1], FlowSequenceEntry::Item(_)));
}

#[test]
fn flow_collection_key_with_comment_before_value() {
    // The `:` after a flow collection key may be separated by comments
    // and line breaks; this is still one pair.
    let allocator = Allocator::default();
    let source = "{[\"key\"] # c\n:value}\n";
    let root = parse(&allocator, source);
    let Content::FlowMapping(mapping) = &body(&root).content else {
        panic!("expected flow mapping");
    };
    assert_eq!(mapping.children.len(), 1);
    let item = &mapping.children[0];
    let key = item.key.as_ref().unwrap().content.as_ref().unwrap();
    assert!(matches!(key.content, Content::FlowSequence(_)));
    assert_eq!(value_node(item).span.slice(source), "value");
    assert_eq!(item.value.as_ref().unwrap().span.slice(source), ":value");
    assert_eq!(root.comments[0].span.slice(source), "# c");
}

#[test]
fn syntax_error_is_fail_fast() {
    let allocator = Allocator::default();
    let error = Parser::new(&allocator, "a:\n\tb: 1\n").parse().unwrap_err();
    assert!(!error.span.is_empty() || error.span.start > 0);
}

#[test]
fn s7bg_colon_followed_by_flow_indicator_in_block() {
    // yaml-test-suite S7BG: `:,` is a valid plain scalar in block context.
    let allocator = Allocator::default();
    let source = "---\n- :,\n";
    let root = parse(&allocator, source);
    let Content::Sequence(seq) = &body(&root).content else {
        panic!("expected sequence");
    };
    let item = seq.children[0].content.as_ref().unwrap();
    assert_eq!(item.span.slice(source), ":,");
}

#[test]
fn directive_with_glued_hash() {
    // A `#` not preceded by whitespace is directive content, not a comment
    // (regression: this used to hang the scanner's bulk word run).
    let allocator = Allocator::default();
    let source = "%TAG !e! tag:example.com,2000:app/#anchor\n---\nx\n";
    let root = parse(&allocator, source);
    let directives = &root.children[0].head.directives;
    assert_eq!(directives[0].parameters.as_slice(), ["!e!", "tag:example.com,2000:app/#anchor"]);
    assert!(root.comments.is_empty());

    let source = "%FOO bar#baz # real comment\n---\nx\n";
    let root = parse(&allocator, source);
    assert_eq!(root.children[0].head.directives[0].parameters.as_slice(), ["bar#baz"]);
    assert_eq!(root.comments.len(), 1);
}

#[test]
fn double_quoted_surrogate_escapes_are_accepted() {
    // yaml@2 and PyYAML accept surrogate pairs and lone surrogates; libyaml rejects both.
    let allocator = Allocator::default();
    for source in [r#""\uD834\uDD1E""#, r#""\uD800""#, r#""\U0001D11E""#] {
        assert!(Parser::new(&allocator, source).parse().is_ok(), "{source}");
    }
    // Beyond the Unicode range is still invalid.
    assert!(Parser::new(&allocator, r#""\U00110000""#).parse().is_err());
}
