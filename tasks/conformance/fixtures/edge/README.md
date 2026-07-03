# Edge fixtures

Hand-written fixtures for input acceptance that Prettier (yaml@2.9 with
`uniqueKeys: false`, `merge: true`, warnings ignored) tolerates but a strict
YAML 1.2 parser might reject. All of these must parse successfully; they
encode the "warning-class" tolerances discovered while auditing Prettier's
YAML implementation:

- unknown directives (`%FOO`) and unsupported `%YAML` versions
- unresolved tags (`!secret`, `!reference`, `!!bar`, wrong-kind tags)
- anchors with unusual but spec-legal names (`&a*b!cd`, `&a:`)
- duplicate mapping keys (incl. Azure Pipelines `${{ if }}` templates)
- YAML 1.1 merge keys (`<<`)
- non-printable characters (no diagnostics in yaml@2)
- plain scalars starting with `:` followed by a flow indicator in block
  context (`- :,`, yaml-test-suite S7BG — the case yaml_parser@0.3.0 rejects)
