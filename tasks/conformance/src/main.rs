//! Conformance checker for `oxc-yaml-parser`.
//!
//! Clones upstream YAML corpora, each pinned to a fixed commit SHA, runs every
//! YAML file through the parser and writes committed snapshot files under
//! `tasks/conformance/snapshots/`:
//!
//! - `summary.snap` — success/failed counts per suite + total.
//! - `<suite>.snap` — the sorted list of files that failed in that suite.
//!
//! `success` is a clean parse; `failed` is `hard_error + panic` on files that
//! are expected to parse. Files the suite itself marks invalid (yaml-test-suite
//! cases with a sibling `error` file, Prettier's `_errors_` fixtures) are
//! *expected* to fail: a parse error there is correct behavior (though this
//! parser is allowed to accept them — rejecting invalid YAML is not a goal),
//! so they are counted in separate `rejected`/`accepted` columns and listed in
//! their own snapshot section.
//!
//! Pinned SHAs keep runs reproducible; bump them deliberately to ingest
//! upstream changes. Cloned repos live under `tasks/conformance/repos/` (git
//! ignored) and are fetched shallow + sparse to stay small. Local fixtures
//! under `tasks/conformance/fixtures/` are committed and always run.
//!
//! ```text
//! cargo run -p conformance                   # clone + parse all suites, write snapshots
//! cargo run -p conformance -- prettier       # only the named suite(s) (summary not rewritten)
//! cargo run -p conformance -- --clone        # clone/update only, do not parse
//! cargo run -p conformance -- --clean        # remove all cloned repos
//! ```

use std::{
    fmt::Write as _,
    fs,
    io::{self, Write},
    panic,
    path::{Path, PathBuf},
    process::Command,
};

use oxc_yaml_parser::{Allocator, Parser};

/// An upstream test corpus, pinned to a fixed commit.
struct Suite {
    /// Directory name under `tasks/conformance/repos/` (or `fixtures/` for
    /// local suites), and the CLI selector.
    name: &'static str,
    /// Git remote to clone from. Empty for local fixture suites.
    url: &'static str,
    /// Pinned commit SHA. Bump deliberately to ingest upstream changes.
    sha: &'static str,
    /// Cone-mode sparse-checkout directories; empty means a full checkout.
    sparse: &'static [&'static str],
    /// Sub-path (relative to the repo root) scanned for YAML files.
    walk: &'static str,
    /// When set, only files with this exact name count as inputs under test
    /// (e.g. yaml-test-suite's `in.yaml`; its `out.yaml`/`emit.yaml` are
    /// expected outputs).
    input_filename: Option<&'static str>,
    /// How to recognize inputs that the suite itself marks as invalid.
    invalid_rule: InvalidRule,
    /// Note shown in the report.
    note: &'static str,
}

/// How a suite marks an input as expected-invalid.
enum InvalidRule {
    /// All inputs are valid.
    None,
    /// A file with this name next to the input marks it invalid.
    SiblingFile(&'static str),
    /// Inputs under a path component with this name are invalid.
    PathComponent(&'static str),
}

const SUITES: &[Suite] = &[
    Suite {
        name: "yaml-test-suite",
        url: "https://github.com/yaml/yaml-test-suite.git",
        // The generated `data` branch: one directory per case with `in.yaml`,
        // and an `error` marker file for invalid-input cases.
        sha: "6ad3d2c62885d82fc349026c136ef560838fdf3d",
        sparse: &[],
        walk: "",
        input_filename: Some("in.yaml"),
        invalid_rule: InvalidRule::SiblingFile("error"),
        note: "official YAML test suite (data branch); error-marked cases are expected-invalid",
    },
    Suite {
        name: "prettier",
        url: "https://github.com/prettier/prettier.git",
        sha: "cf7db3500a89faeb24ad0af45c6b9a0e7b074a03",
        sparse: &["tests/format/yaml"],
        walk: "tests/format/yaml",
        input_filename: None,
        invalid_rule: InvalidRule::PathComponent("_errors_"),
        note: "Prettier's YAML fixtures (3.9-era, yaml@2 parser); _errors_ are expected-invalid",
    },
    Suite {
        name: "edge",
        url: "",
        sha: "",
        sparse: &[],
        walk: "",
        input_filename: None,
        invalid_rule: InvalidRule::None,
        note: "local fixtures: Prettier warning-class tolerances (see fixtures/edge/README.md)",
    },
];

fn repos_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("repos")
}

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

fn snapshots_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("snapshots")
}

fn suite_root(suite: &Suite) -> PathBuf {
    if suite.url.is_empty() {
        fixtures_dir().join(suite.name)
    } else {
        repos_dir().join(suite.name)
    }
}

fn git(dir: &Path, args: &[&str]) -> io::Result<std::process::Output> {
    Command::new("git").arg("-C").arg(dir).args(args).output()
}

fn git_ok(dir: &Path, args: &[&str]) -> io::Result<()> {
    let output = git(dir, args)?;
    if output.status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "`git {}` failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )))
    }
}

/// Clone or update `suite` to its pinned SHA. Returns `Ok(true)` if a network
/// fetch happened, `Ok(false)` if the checkout was already at the pinned SHA.
fn ensure_repo(suite: &Suite) -> io::Result<bool> {
    let dir = repos_dir().join(suite.name);

    if !dir.join(".git").is_dir() {
        fs::create_dir_all(&dir)?;
        git_ok(&dir, &["init", "-q"])?;
    }

    let has_origin = git(&dir, &["remote", "get-url", "origin"]).is_ok_and(|o| o.status.success());
    if !has_origin {
        git_ok(&dir, &["remote", "add", "origin", suite.url])?;
    }

    // Already checked out at the pinned SHA — nothing to do.
    if let Ok(out) = git(&dir, &["rev-parse", "HEAD"])
        && out.status.success()
        && String::from_utf8_lossy(&out.stdout).trim() == suite.sha
    {
        return Ok(false);
    }

    let sparse = !suite.sparse.is_empty();
    if sparse {
        git_ok(&dir, &["sparse-checkout", "init", "--cone"])?;
        let mut args = vec!["sparse-checkout", "set"];
        args.extend_from_slice(suite.sparse);
        git_ok(&dir, &args)?;
    }

    let mut fetch = vec!["fetch", "-q", "--depth", "1"];
    if sparse {
        fetch.push("--filter=blob:none");
    }
    fetch.extend_from_slice(&["origin", suite.sha]);
    git_ok(&dir, &fetch)?;
    git_ok(&dir, &["checkout", "-q", "FETCH_HEAD"])?;
    Ok(true)
}

/// The outcome of parsing one file.
enum Outcome {
    Clean,
    HardError(String),
    Panic,
}

fn parse_outcome(source: &str) -> Outcome {
    let caught = panic::catch_unwind(|| {
        let allocator = Allocator::default();
        let parser = Parser::new(&allocator, source);
        match parser.parse() {
            Ok(_) => Outcome::Clean,
            Err(error) => Outcome::HardError(format!("{:?}", error.kind)),
        }
    });
    caught.unwrap_or(Outcome::Panic)
}

fn is_yaml(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == "yaml" || ext == "yml")
}

fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        // `DirEntry::file_type` is free on unix (readdir's d_type) — no extra
        // stat syscalls per entry.
        let Ok(file_type) = entry.file_type() else { continue };
        // Skip symlinks: the yaml-test-suite data branch aliases case dirs
        // under `tags/` and `name/` via symlinks; following them would count
        // cases more than once.
        if file_type.is_symlink() {
            continue;
        }
        let path = entry.path();
        if file_type.is_dir() {
            let skip =
                path.file_name().is_some_and(|name| name == ".git" || name == "__snapshots__");
            if !skip {
                collect_files(&path, out);
            }
        } else if is_yaml(&path) {
            out.push(path);
        }
    }
}

/// Whether this file is expected (allowed) to fail to parse.
fn expects_error(path: &Path, suite: &Suite) -> bool {
    match suite.invalid_rule {
        InvalidRule::None => false,
        InvalidRule::SiblingFile(marker) => {
            path.parent().is_some_and(|dir| dir.join(marker).exists())
        }
        InvalidRule::PathComponent(component) => {
            path.components().any(|c| c.as_os_str() == component)
        }
    }
}

/// Whether this file is an input under test (see [`Suite::input_filename`]).
fn is_input(path: &Path, suite: &Suite) -> bool {
    suite.input_filename.is_none_or(|name| path.file_name().is_some_and(|n| n == name))
}

#[derive(Default)]
struct Tally {
    files: u32,
    clean: u32,
    hard_error: u32,
    panic: u32,
    /// Outcomes on expected-invalid files (not counted in the above).
    rejected: u32,
    accepted: u32,
    invalid_panic: u32,
}

impl Tally {
    fn add(&mut self, other: &Tally) {
        self.files += other.files;
        self.clean += other.clean;
        self.hard_error += other.hard_error;
        self.panic += other.panic;
        self.rejected += other.rejected;
        self.accepted += other.accepted;
        self.invalid_panic += other.invalid_panic;
    }

    fn success(&self) -> u32 {
        self.clean
    }

    fn failed(&self) -> u32 {
        self.hard_error + self.panic
    }
}

/// One failing file: `tag` is `ERROR`/`PANIC`/`ACCEPT`, `rel_path` is relative
/// to the suite root, `label` is the error kind (empty otherwise).
struct Failure {
    tag: &'static str,
    rel_path: String,
    label: String,
}

#[derive(Default)]
struct SuiteReport {
    tally: Tally,
    failures: Vec<Failure>,
    /// Outcomes on expected-invalid files, snapshotted in their own section:
    /// a rejected case flipping to accepted (or vice versa) surfaces in review.
    invalid_outcomes: Vec<Failure>,
}

/// Render a path relative to `base` using forward slashes.
fn rel_path(path: &Path, base: &Path) -> String {
    let rel = path.strip_prefix(base).unwrap_or(path);
    rel.components().filter_map(|c| c.as_os_str().to_str()).collect::<Vec<_>>().join("/")
}

fn run_suite(suite: &Suite) -> SuiteReport {
    let root = suite_root(suite);
    let mut files = Vec::new();
    collect_files(&root.join(suite.walk), &mut files);
    files.sort();

    let mut report = SuiteReport::default();
    for path in files {
        if !is_input(&path, suite) {
            continue;
        }
        let Ok(bytes) = fs::read(&path) else { continue };
        let Ok(source) = String::from_utf8(bytes) else { continue };
        let rel = rel_path(&path, &root);
        let expect_error = expects_error(&path, suite);
        let outcome = parse_outcome(&source);

        let t = &mut report.tally;
        t.files += 1;
        if expect_error {
            match outcome {
                Outcome::Clean => {
                    t.accepted += 1;
                    report.invalid_outcomes.push(Failure {
                        tag: "ACCEPT",
                        rel_path: rel,
                        label: String::new(),
                    });
                }
                Outcome::HardError(_) => t.rejected += 1,
                Outcome::Panic => {
                    t.invalid_panic += 1;
                    report.invalid_outcomes.push(Failure {
                        tag: "PANIC",
                        rel_path: rel,
                        label: String::new(),
                    });
                }
            }
        } else {
            match outcome {
                Outcome::Clean => t.clean += 1,
                Outcome::HardError(label) => {
                    t.hard_error += 1;
                    report.failures.push(Failure { tag: "ERROR", rel_path: rel, label });
                }
                Outcome::Panic => {
                    t.panic += 1;
                    report.failures.push(Failure {
                        tag: "PANIC",
                        rel_path: rel,
                        label: String::new(),
                    });
                }
            }
        }
    }
    report.failures.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    report.invalid_outcomes.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    report
}

fn header_row(first: &str) -> String {
    format!(
        "{first:<18} {:>6} {:>8} {:>7} {:>6} {:>9} {:>9} {:>9}",
        "files", "success", "failed", "panic", "rejected", "accepted", "inv_panic"
    )
}

fn row(label: &str, t: &Tally) -> String {
    format!(
        "{label:<18} {:>6} {:>8} {:>7} {:>6} {:>9} {:>9} {:>9}",
        t.files,
        t.success(),
        t.failed(),
        t.panic,
        t.rejected,
        t.accepted,
        t.invalid_panic
    )
}

fn write_failure_lines(out: &mut String, failures: &[Failure]) {
    if failures.is_empty() {
        let _ = writeln!(out, "none");
    }
    for failure in failures {
        if failure.label.is_empty() {
            let _ = writeln!(out, "{:<8} {}", failure.tag, failure.rel_path);
        } else {
            let _ = writeln!(out, "{:<8} {}    {}", failure.tag, failure.rel_path, failure.label);
        }
    }
}

fn write_suite_snapshot(suite: &Suite, report: &SuiteReport) -> io::Result<()> {
    let t = &report.tally;
    let mut out = String::new();
    let _ = writeln!(out, "suite: {}", suite.name);
    if !suite.sha.is_empty() {
        let _ = writeln!(out, "sha: {}", suite.sha);
    }
    let _ = writeln!(
        out,
        "files: {}   success: {}   failed: {}   rejected: {}   accepted: {}",
        t.files,
        t.success(),
        t.failed(),
        t.rejected,
        t.accepted,
    );
    let _ = writeln!(out, "\nfailures:");
    write_failure_lines(&mut out, &report.failures);
    if t.rejected + t.accepted + t.invalid_panic > 0 {
        let _ = writeln!(
            out,
            "\naccepted invalid inputs (rejecting invalid YAML is not a goal; tracked for review):"
        );
        write_failure_lines(&mut out, &report.invalid_outcomes);
    }
    fs::write(snapshots_dir().join(format!("{}.snap", suite.name)), out)
}

fn write_summary_snapshot(reports: &[(&Suite, SuiteReport)], total: &Tally) -> io::Result<()> {
    let mut out = String::new();
    let _ = writeln!(out, "# oxc-yaml-parser conformance — `cargo run -p conformance`");
    let _ = writeln!(out, "# success = clean parse; failed = hard_error + panic;");
    let _ = writeln!(
        out,
        "# rejected/accepted = outcomes on expected-invalid inputs (either is acceptable, panics are not)"
    );
    let _ = writeln!(out);
    let _ = writeln!(out, "{}", header_row("suite"));
    for (suite, report) in reports {
        let _ = writeln!(out, "{}", row(suite.name, &report.tally));
    }
    let _ = writeln!(out, "{}", "-".repeat(84));
    let _ = writeln!(out, "{}", row("total", total));
    fs::write(snapshots_dir().join("summary.snap"), out)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let clean = args.iter().any(|a| a == "--clean");
    let clone_only = args.iter().any(|a| a == "--clone");
    let filters: Vec<&str> =
        args.iter().filter(|a| !a.starts_with('-')).map(String::as_str).collect();

    if clean {
        let dir = repos_dir();
        match fs::remove_dir_all(&dir) {
            Ok(()) => println!("removed {}", dir.display()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => println!("nothing to remove"),
            Err(e) => eprintln!("failed to remove {}: {e}", dir.display()),
        }
        return;
    }

    let selected: Vec<&Suite> =
        SUITES.iter().filter(|s| filters.is_empty() || filters.contains(&s.name)).collect();
    if selected.is_empty() {
        let names = SUITES.iter().map(|s| s.name).collect::<Vec<_>>().join(", ");
        eprintln!("no matching suite; available: {names}");
        return;
    }

    // Silence per-file panic output; `catch_unwind` records it instead.
    panic::set_hook(Box::new(|_| {}));

    println!("cloning into {}", repos_dir().display());
    let mut clone_failed = false;
    for suite in &selected {
        if suite.url.is_empty() {
            continue;
        }
        print!("  {:<18} {}  ", suite.name, &suite.sha[..12]);
        io::stdout().flush().ok();
        match ensure_repo(suite) {
            Ok(true) => println!("fetched"),
            Ok(false) => println!("up-to-date"),
            Err(e) => {
                println!("ERROR: {e}");
                clone_failed = true;
            }
        }
    }
    if clone_failed {
        eprintln!("\none or more clones failed (network?); re-run to retry.");
        std::process::exit(1);
    }

    if clone_only {
        return;
    }

    let full_run = filters.is_empty();
    let mut total = Tally::default();
    let mut reports: Vec<(&Suite, SuiteReport)> = Vec::new();
    for suite in &selected {
        let report = run_suite(suite);
        total.add(&report.tally);
        reports.push((suite, report));
    }

    println!("\n{}", header_row("suite"));
    for (suite, report) in &reports {
        println!("{}", row(suite.name, &report.tally));
    }
    println!("{}", "-".repeat(84));
    println!("{}", row("total", &total));
    println!("\nnotes:");
    for (suite, _) in &reports {
        println!("  {:<18} {}", suite.name, suite.note);
    }

    if let Err(e) = fs::create_dir_all(snapshots_dir()) {
        eprintln!("failed to create {}: {e}", snapshots_dir().display());
        return;
    }
    for (suite, report) in &reports {
        if let Err(e) = write_suite_snapshot(suite, report) {
            eprintln!("failed to write {}.snap: {e}", suite.name);
        }
    }
    if full_run {
        if let Err(e) = write_summary_snapshot(&reports, &total) {
            eprintln!("failed to write summary.snap: {e}");
        }
    } else {
        println!("\n(partial run — summary.snap left unchanged)");
    }

    println!("\nsnapshots written to {}", snapshots_dir().display());
}
