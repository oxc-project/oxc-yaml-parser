# oxc-yaml-parser tasks. Run `just` (or `just --list`) to see all recipes.

# List available recipes
_default:
    @just --list

# Run the test suite (mirrors CI)
test:
    cargo test --all-features

# Format the codebase
fmt:
    cargo fmt

# Clone the conformance suites (pinned SHAs), parse them, and regenerate the
# committed snapshots under tasks/conformance/snapshots/. Review with `git diff`.
# Optionally restrict to named suites, e.g. `just conformance yaml-test-suite`
# (a filtered run updates only those suites' snapshots, not summary.snap).
conformance *suites:
    cargo run -p conformance -- {{ suites }}

# Clone/update the conformance suites without parsing them.
conformance-clone *suites:
    cargo run -p conformance -- --clone {{ suites }}

# Remove all cloned conformance repos.
conformance-clean:
    cargo run -p conformance -- --clean
