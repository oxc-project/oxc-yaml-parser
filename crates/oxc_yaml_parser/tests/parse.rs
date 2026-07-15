use oxc_yaml_parser::{Allocator, Parser, ast::*};

fn parse<'a>(allocator: &'a Allocator, source: &'a str) -> Root<'a> {
    Parser::new(allocator, source).parse().unwrap()
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
fn plain_scalar_span_excludes_trailing_whitespace() {
    let allocator = Allocator::default();
    let source = "key: value  \n";
    let root = parse(&allocator, source);
    let Some(Content::Mapping(mapping)) = &root.children[0].body.content else {
        panic!("expected mapping");
    };
    let value = mapping.children[0].value.content.as_ref().unwrap();
    assert_eq!(value.span().slice(source), "value");
}

#[test]
fn multiline_plain_scalar_span() {
    let allocator = Allocator::default();
    let source = "key: one\n  two\n";
    let root = parse(&allocator, source);
    let Some(Content::Mapping(mapping)) = &root.children[0].body.content else {
        panic!("expected mapping");
    };
    let value = mapping.children[0].value.content.as_ref().unwrap();
    assert_eq!(value.span().slice(source), "one\n  two");
}

#[test]
fn block_scalar_header() {
    let allocator = Allocator::default();
    let source = "key: |2+\n  text\n";
    let root = parse(&allocator, source);
    let Some(Content::Mapping(mapping)) = &root.children[0].body.content else {
        panic!("expected mapping");
    };
    let Some(Content::BlockLiteral(block)) = &mapping.children[0].value.content else {
        panic!("expected block literal");
    };
    assert_eq!(block.chomping, Chomping::Keep);
    assert_eq!(block.indent, Some(2));
    assert_eq!(&source[block.content_start as usize..block.span.end as usize], "  text\n");
}

#[test]
fn anchor_tag_and_alias() {
    let allocator = Allocator::default();
    let source = "a: &x !!str hello\nb: *x\n";
    let root = parse(&allocator, source);
    let Some(Content::Mapping(mapping)) = &root.children[0].body.content else {
        panic!("expected mapping");
    };
    let a = mapping.children[0].value.content.as_ref().unwrap();
    assert_eq!(a.props().anchor.unwrap().span.slice(source), "&x");
    assert_eq!(a.props().tag.unwrap().span.slice(source), "!!str");
    let b = mapping.children[1].value.content.as_ref().unwrap();
    assert!(matches!(b, Content::Alias(_)));
}

#[test]
fn explicit_key() {
    let allocator = Allocator::default();
    let source = "? key\n: value\n";
    let root = parse(&allocator, source);
    let Some(Content::Mapping(mapping)) = &root.children[0].body.content else {
        panic!("expected mapping");
    };
    let key = &mapping.children[0].key;
    assert!(key.explicit);
    // An explicit key's span starts at the `?` indicator
    // (mirrors yaml-unist-parser's mappingKey range).
    assert_eq!(key.span.slice(source), "? key");
}

#[test]
fn explicit_key_without_content() {
    let allocator = Allocator::default();
    let source = "?\n: value\n";
    let root = parse(&allocator, source);
    let Some(Content::Mapping(mapping)) = &root.children[0].body.content else {
        panic!("expected mapping");
    };
    let key = &mapping.children[0].key;
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
    let Some(Content::Mapping(mapping)) = &root.children[0].body.content else {
        panic!("expected mapping");
    };
    let Some(Content::Sequence(seq)) = &mapping.children[0].value.content else {
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
    let Some(Content::Sequence(seq)) = &root.children[0].body.content else {
        panic!("expected sequence");
    };
    assert_eq!(seq.children.len(), 2);
    let first = seq.children[0].content.as_ref().unwrap();
    assert!(matches!(first, Content::Plain(_)));
    assert_eq!(first.props().tag.unwrap().span.slice(source), "!!tag");
    assert!(first.span().is_empty());
}

#[test]
fn flow_pair_in_sequence() {
    let allocator = Allocator::default();
    let source = "[a: b, c]\n";
    let root = parse(&allocator, source);
    let Some(Content::FlowSequence(seq)) = &root.children[0].body.content else {
        panic!("expected flow sequence");
    };
    assert_eq!(seq.children.len(), 2);
    assert!(matches!(seq.children[0], FlowSequenceEntry::Pair(_)));
    assert!(matches!(seq.children[1], FlowSequenceEntry::Item(_)));
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
    let Some(Content::Sequence(seq)) = &root.children[0].body.content else {
        panic!("expected sequence");
    };
    let item = seq.children[0].content.as_ref().unwrap();
    assert_eq!(item.span().slice(source), ":,");
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
