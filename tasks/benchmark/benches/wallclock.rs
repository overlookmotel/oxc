use std::{env, fs, path::PathBuf, time::Duration};

use oxc_benchmark::{
    black_box, criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion, SamplingMode,
};

use serde::Deserialize;

#[derive(Deserialize)]
struct BenchResult {
    filename: String,
    duration: f64,
}

/// This is a fake benchmark which is only here to get benchmarks for NAPI parser into CodSpeed.
/// It's a workaround for CodSpeed's measurement of JS + NAPI being wildly inaccurate:
/// https://github.com/CodSpeedHQ/action/issues/96
/// So instead in CI we run the actual benchmark without CodSpeed's instrumentation
/// (see `.github/workflows/benchmark.yml` and `napi/parser/parse.bench.mjs`).
/// `parse.bench.mjs` writes the results of the benchmarks to a file `results.json`.
/// This pseudo-benchmark reads that file and just performs meaningless calculations in a loop
/// the number of times required to take same amount of time as the original benchmark.
fn bench_wallclock(criterion: &mut Criterion) {
    let data_dir = env::var("DATA_DIR").unwrap();
    let results_path: PathBuf = [&data_dir, "results.json"].iter().collect();
    let results_file = fs::File::open(&results_path).unwrap();
    let files: Vec<BenchResult> = serde_json::from_reader(results_file).unwrap();
    fs::remove_file(&results_path).unwrap();

    let group_name = match env::var("DESERIALIZE_ONLY") {
        Ok(deser) if deser == "true" => "parser_napi_deser",
        _ => "parser_napi",
    };

    let mut group = criterion.benchmark_group(group_name);
    // Reduce time to run benchmark as much as possible (10 is min for sample size)
    group.sample_size(10);
    group.warm_up_time(Duration::from_micros(1));
    group.sampling_mode(SamplingMode::Flat);
    for file in files {
        group.bench_function(BenchmarkId::from_parameter(&file.filename), |b| {
            b.iter_batched(
                || black_box(0x1c2e9b89d37e0c1b),
                wallclock_run,
                BatchSize::PerIteration,
            );
        });
    }
    group.finish();
}

#[inline(never)]
fn wallclock_run(n: u64) -> u64 {
    n ^ 0x18bb6752b938b511
}

criterion_group!(wallclock, bench_wallclock);
criterion_main!(wallclock);
