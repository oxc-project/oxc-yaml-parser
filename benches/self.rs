use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use oxc_yaml_parser::{Allocator, Parser};
use std::{
    fs,
    hint::black_box,
    path::{Path, PathBuf},
    time::Duration,
};

fn bench_parser(c: &mut Criterion) {
    let mut group = c.benchmark_group("self");
    group.measurement_time(Duration::from_secs(8));

    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let bench_data_dir = workspace_root.join("bench_data");
    let fixture_dir = workspace_root.join("benchmark/fixtures");
    let mut inputs = collect_bench_inputs(&bench_data_dir);
    if inputs.is_empty() {
        inputs = collect_bench_inputs(&fixture_dir);
    }
    assert!(
        !inputs.is_empty(),
        "no benchmark inputs found in {} or {}",
        bench_data_dir.display(),
        fixture_dir.display(),
    );

    for path in inputs {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let code = black_box(fs::read_to_string(&path).unwrap_or_else(|error| {
            panic!("failed to read benchmark input {}: {error}", path.display())
        }));

        group.bench_with_input(BenchmarkId::from_parameter(name), &code, |b, code| {
            b.iter(|| {
                let allocator = Allocator::default();
                let parser = Parser::new(&allocator, code);
                black_box(parser.parse().unwrap());
            });
        });
    }
    group.finish();
}

fn collect_bench_inputs(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut inputs: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file() && path.extension().is_some_and(|ext| ext == "yaml" || ext == "yml")
        })
        .collect();
    inputs.sort();
    inputs
}

criterion_group!(benches, bench_parser);
criterion_main!(benches);
