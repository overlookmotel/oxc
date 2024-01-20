use oxc_allocator::Allocator;
use oxc_benchmark::{criterion_group, criterion_main, BenchmarkId, Criterion};
use oxc_parser::__lexer::{Kind, Lexer};
use oxc_span::SourceType;
use oxc_tasks_common::TestFiles;

fn bench_lexer(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("lexer");
    for file in TestFiles::complicated().files() {
        let source_type = SourceType::from_path(&file.file_name).unwrap();
        group.bench_with_input(
            BenchmarkId::from_parameter(&file.file_name),
            &file.source_text,
            |b, source_text| {
                b.iter_with_large_drop(|| {
                    // Include the allocator drop time to make time measurement consistent.
                    // Otherwise the allocator will allocate huge memory chunks (by power of two) from the
                    // system allocator, which makes time measurement unequal during long runs.
                    let allocator = Allocator::default();
                    let mut lexer = Lexer::new(&allocator, source_text, source_type);
                    while lexer.next_token().kind != Kind::Eof {}
                    allocator
                });
            },
        );
    }
    group.finish();
}

criterion_group!(lexer, bench_lexer);
criterion_main!(lexer);
