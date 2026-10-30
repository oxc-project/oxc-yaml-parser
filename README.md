# oxc-yaml-parser

`oxc-yaml-parser` parses YAML 1.2 into a comment-preserving, span-faithful typed AST, designed for building formatters.

How it parses, and why, is in [AGENTS.md](https://github.com/oxc-project/oxc-yaml-parser/blob/main/AGENTS.md).

## Example

```rust
use oxc_yaml_parser::{Allocator, Parser};

let allocator = Allocator::default();
let parser = Parser::new(&allocator, "key: value # comment");
let root = parser.parse().unwrap();
println!("{root:#?}");
```

More examples are available in [`examples`](https://github.com/oxc-project/oxc-yaml-parser/tree/main/examples).

## License

MIT

The scanner derives from [libyaml](https://github.com/yaml/libyaml) (MIT) by way of [saphyr](https://github.com/saphyr-rs/saphyr) (MIT); their copyright notices are included in [LICENSE](https://github.com/oxc-project/oxc-yaml-parser/blob/main/LICENSE).

# [Sponsored By](https://oxc.rs/sponsor)

<p align="center">
  <a href="https://oxc.rs/sponsor">
    <img src="https://raw.githubusercontent.com/oxc-project/sponsors/main/sponsors.svg" alt="Our sponsors" />
  </a>
</p>
