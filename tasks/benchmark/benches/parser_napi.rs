use std::{
    env, fs,
    path::PathBuf,
    time::{Duration, Instant},
};

use oxc_benchmark::{criterion_group, criterion_main, BenchmarkId, Criterion};

use serde::Deserialize;

#[derive(Deserialize)]
struct BenchResult {
    filename: String,
    duration: f64,
}

/// This is a fake benchmark which is only here to get benchmarks for NAPI parser into CodSpeed.
/// It's a workaround for CodSpeed's measurement of JS + NAPI being wildly inaccurate.
/// https://github.com/CodSpeedHQ/action/issues/96
/// So instead in CI we run the actual benchmark outside CodSpeed's instrumentation
/// (see `.github/workflows/benchmark.yml` and `napi/parser/parse.bench.mjs`).
/// `parse.bench.mjs` writes the measurements for the benchmarks to a file `results.json`.
/// This pseudo-benchmark reads that file and just busy-loops for the specified time.
fn bench_parser_napi(criterion: &mut Criterion) {
    let data_dir = env::var("DATA_DIR").unwrap();
    let results_path: PathBuf = [&data_dir, "results.json"].iter().collect();
    let results_file = fs::File::open(&results_path).unwrap();
    let files: Vec<BenchResult> = serde_json::from_reader(results_file).unwrap();
    fs::remove_file(&results_path).unwrap();

    let mut group = criterion.benchmark_group("parser_napi");
    // Reduce time to run benchmark as much as possible (10 is min for sample size)
    group.sample_size(10);
    group.warm_up_time(Duration::from_micros(1));
    for file in files {
        let duration = Duration::from_secs_f64(file.duration);
        println!("intended duration: {} = {:?}", &file.filename, duration);
        group.bench_function(BenchmarkId::from_parameter(&file.filename), |b| {
            b.iter(|| {
                let start = Instant::now();
                while start.elapsed() < duration {}
            });
            // b.iter_custom(|iters| duration.mul_f64(iters as f64));
        });
    }
    group.finish();
}

criterion_group!(parser, bench_parser_napi);
criterion_main!(parser);
