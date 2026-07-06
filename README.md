# oxc-yaml-parser

`oxc-yaml-parser` parses YAML 1.2 into a comment-preserving, span-faithful typed AST, designed for building formatters.

- The AST mirrors [yaml-unist-parser](https://github.com/prettier/yaml-unist-parser)'s node shapes — the AST Prettier's YAML printer consumes.
- Scalar values are not cooked: consumers slice the original source through spans.
- Comments are retained as trivia with exact spans.
- Syntax-only: no schema/tag resolution, no anchor/alias resolution, no value typing.
- Fail-fast: a syntax error returns `Err` with a span; no partial AST is produced.

The scanner is a port of the libyaml scanning algorithm (by way of [saphyr](https://github.com/saphyr-rs/saphyr)), adapted to byte-offset spans and trivia retention.

## Example

```rust
use oxc_yaml_parser::{Allocator, Parser};

let allocator = Allocator::default();
let parser = Parser::new(&allocator, "key: value # comment");
let root = parser.parse().unwrap();
println!("{root:#?}");
```

More examples are available in [`examples`](./examples).

## Conformance

`cargo run -p conformance` (or `just conformance`) parses three corpora and snapshots the results under [`tasks/conformance/snapshots`](./tasks/conformance/snapshots):

- the official [yaml-test-suite](https://github.com/yaml/yaml-test-suite) (`data` branch, including its invalid-input cases),
- [Prettier](https://github.com/prettier/prettier)'s YAML format fixtures,
- local edge fixtures covering Prettier's input-acceptance tolerances.

All valid inputs in all three corpora parse successfully. Rejecting invalid YAML is a non-goal; outcomes on invalid inputs are tracked in the snapshots for review.

## License

MIT

The scanner derives from [libyaml](https://github.com/yaml/libyaml) (MIT) by way of [saphyr](https://github.com/saphyr-rs/saphyr) (MIT); their copyright notices are included in [LICENSE](./LICENSE).

## ❤ Who's [Sponsoring Oxc](https://github.com/sponsors/Boshen)?

<p align="center">
  <a href="https://github.com/sponsors/Boshen">
    <img src="https://raw.githubusercontent.com/Boshen/sponsors/main/sponsors.svg" alt="Our sponsors" />
  </a>
</p>
