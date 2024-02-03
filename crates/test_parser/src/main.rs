#![allow(unused_imports)]

use std::{fs::read_to_string, path::PathBuf};

extern crate oxc_allocator;
extern crate oxc_parser;
extern crate oxc_semantic;
extern crate oxc_span;

use oxc_allocator::Allocator;
use oxc_parser::Parser;
use oxc_semantic::SemanticBuilder;
use oxc_span::SourceType;

fn main() {
    let path = "/Users/jim/Downloads/oxc benchmarks/pdf.mjs";
    let source_text = read_to_string(path).unwrap();

    let mut allocator = Allocator::default();
    let source_type = SourceType::from_path(path).unwrap();

    {
        let ret = Parser::new(&allocator, &source_text, source_type).parse();
        let program = allocator.alloc(ret.program);

        println!("----------------------------------------");
        println!("Program:");
        println!("{:#?}", program);

        println!("----------------------------------------");
        println!("Trivias:");
        println!("{:#?}", ret.trivias);

        println!("----------------------------------------");
        println!("Errors:");
        for error in ret.errors {
            let error = error.with_source_code(source_text.clone());
            println!("{error:?}");
        }

        let sem = SemanticBuilder::new(&source_text, source_type)
            .build_module_record(PathBuf::new(), program)
            .build(program);
        println!("----------------------------------------");
        println!("Semantic errors:");
        println!("{:#?}", sem.errors);
        println!("Semantic nodes:");
        println!("{:#?}", sem.semantic.nodes().iter().collect::<Vec<_>>().len());
        println!("Semantic scopes:");
        println!("{:#?}", sem.semantic.scopes().len());
        println!("Semantic classes:");
        println!("{:#?}", sem.semantic.classes().iter_enumerated().collect::<Vec<_>>().len());
        println!("Semantic trivias:");
        println!("{:#?}", sem.semantic.trivias().comments().len());
        println!("{:#?}", sem.semantic.trivias().irregular_whitespaces().len());
        println!("Semantic module_record:");
        println!("{:#?}", *sem.semantic.module_record());
        println!("Semantic symbols:");
        println!("{:#?}", sem.semantic.symbols().len());
        println!("Semantic unused_labels:");
        println!("{:#?}", sem.semantic.unused_labels().len());
        // println!("Semantic cfg:");
        // println!("{:#?}", sem.semantic.cfg());
    }

    let bump = &mut *allocator;

    println!("----------------------------------------");
    println!("Allocator:");
    println!("allocated_bytes: {}", bump.allocated_bytes());
    println!("allocated_bytes_including_metadata: {}", bump.allocated_bytes_including_metadata());
    println!("Chunks:");
    for chunk in bump.iter_allocated_chunks() {
        println!("{}", chunk.len());
    }
}
