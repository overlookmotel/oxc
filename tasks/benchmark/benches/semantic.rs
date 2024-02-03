use oxc_allocator::Allocator;
use oxc_benchmark::{criterion_group, criterion_main, BenchmarkId, Criterion};
use oxc_parser::Parser;
use oxc_semantic::SemanticBuilder;
use oxc_span::SourceType;
use oxc_tasks_common::TestFiles;
use std::path::PathBuf;

fn bench_semantic(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("semantic");
    for file in TestFiles::complicated().files() {
        let source_type = SourceType::from_path(&file.file_name).unwrap();
        group.bench_with_input(
            BenchmarkId::from_parameter(&file.file_name),
            &(&file.source_text, &file.file_name),
            |b, (source_text, file_name)| {
                let allocator = Allocator::default();
                let ret = Parser::new(&allocator, source_text, source_type).parse();
                let program = allocator.alloc(ret.program);
                let mut count: usize = 0;
                b.iter_with_large_drop(|| {
                    count += 1;
                    SemanticBuilder::new(source_text, source_type)
                        .build_module_record(PathBuf::new(), program)
                        .build(program)
                });
                eprintln!("Iterations for {file_name}: {count}");
            },
        );
    }
    group.finish();
}

criterion_group!(semantic, bench_semantic);
criterion_main!(semantic);
