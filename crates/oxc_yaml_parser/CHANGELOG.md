# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.0.3](https://github.com/oxc-project/oxc-yaml-parser/compare/oxc-yaml-parser-v0.0.2...oxc-yaml-parser-v0.0.3) - 2026-07-23

### Added

- [**breaking**] redesign AST spans ([#24](https://github.com/oxc-project/oxc-yaml-parser/pull/24))

### Other

- update sponsor section

## [0.0.2](https://github.com/oxc-project/oxc-yaml-parser/compare/oxc-yaml-parser-v0.0.1...oxc-yaml-parser-v0.0.2) - 2026-07-15

### Fixed

- *(scanner)* end block scalar token after its last owned line break ([#20](https://github.com/oxc-project/oxc-yaml-parser/pull/20))
- *(scanner)* allow comments and breaks before `:` after a flow collection key ([#19](https://github.com/oxc-project/oxc-yaml-parser/pull/19))
- *(parser)* start explicit mapping key span at the `?` indicator ([#18](https://github.com/oxc-project/oxc-yaml-parser/pull/18))
- *(parser)* restrict indentless sequences to mapping key/value position ([#17](https://github.com/oxc-project/oxc-yaml-parser/pull/17))

### Other

- *(scanner)* use cast_signed/cast_unsigned for indent casts ([#16](https://github.com/oxc-project/oxc-yaml-parser/pull/16))
- drop redundant size_of import ([#15](https://github.com/oxc-project/oxc-yaml-parser/pull/15))
- *(scanner)* use Vec::pop_if for indent bookkeeping ([#14](https://github.com/oxc-project/oxc-yaml-parser/pull/14))
- declare rust-version 1.95.0 ([#13](https://github.com/oxc-project/oxc-yaml-parser/pull/13))
- update license notice ([#12](https://github.com/oxc-project/oxc-yaml-parser/pull/12))
- *(ast)* move span field first ([#11](https://github.com/oxc-project/oxc-yaml-parser/pull/11))
- extract block scalar header parsing helpers ([#9](https://github.com/oxc-project/oxc-yaml-parser/pull/9))
- remove unused insta dev-dependency ([#8](https://github.com/oxc-project/oxc-yaml-parser/pull/8))
- normalize README sponsor section
