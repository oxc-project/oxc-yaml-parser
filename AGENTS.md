# AGENTS.md - AI Assistant Guide for oxc-yaml-parser

A YAML 1.2 parser producing a comment-preserving, span-faithful typed AST.
It is the parsing layer for oxc's YAML formatter (`oxc_formatter_yaml`).

## Input acceptance

The reference is what [yaml@2](https://github.com/eemeli/yaml) (Prettier's YAML parser) accepts, and how it reads it.

- A NUL is an ordinary character, as in yaml@2 (libyaml rejects it as a control character)
- Stream-level syntax is recognized only where yaml@2 and libyaml agree it can appear
  - `---`/`...` is a document marker only at column 0; indented, it continues a plain scalar
  - A BOM is skipped only outside a document (the stream start, after `...`); inside one it is content (libyaml skips it at any line start)
- What Prettier tolerates beyond a strict YAML 1.2 parser (unknown directives, unresolved tags, duplicate keys, ...) must parse:
  `tasks/conformance/fixtures/edge/README.md` lists them.
- Rejecting invalid YAML is a non-goal.

## Scope

- Node names follow [yaml-unist-parser](https://github.com/prettier/yaml-unist-parser), the AST Prettier's YAML printer consumes;
  the span principles and the departures from it (no cooked values, comments not attached to nodes) are in the header of `src/ast.rs`.
- Syntax-only: no schema/tag resolution, no anchor/alias resolution, no value typing.
- Fail-fast: a syntax error returns `Err` with a span; no partial AST is produced.

## Architecture

- `src/scanner.rs`: a port of libyaml's scanner by way of [saphyr](https://github.com/saphyr-rs/saphyr); its header lists the adaptations,
  and its core machinery is not "improved" without cross-checking against the yaml-test-suite.
- `src/parser.rs`: recursive descent over the scanner's tokens, building the AST.

## Testing

| Command | Covers |
| --- | --- |
| `cargo test` | AST shape and span tests (`tests/parse.rs`), doctests |
| `just conformance [suite]...` | yaml-test-suite (`data` branch), Prettier's YAML format fixtures, local edge fixtures; fetches the suites itself, snapshot-pinned |
| `cargo bench` | parse speed over `benchmark/fixtures` (or a local `bench_data` directory) |

CI (`.github/workflows`) runs check, test, clippy, doc and typos, and fails when the conformance snapshots are stale.

- What the conformance columns mean, how suites are pinned and what a filtered run rewrites: the header of `tasks/conformance/src/main.rs`.
