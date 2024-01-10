use std::path::Path;

extern crate oxc_allocator;
extern crate oxc_parser;
extern crate oxc_span;

use oxc_allocator::Allocator;
use oxc_parser::Parser;
use oxc_span::SourceType;

fn main() {
    parse("x = /y/;");
}

fn parse(source: &str) {
    println!("----------------------------------------");
    println!("Parsing: {}", source);

    let source_text = source.to_string();
    let allocator = Allocator::default();
    let source_type = SourceType::from_path(Path::new("/path/to/foo.js")).unwrap();
    let ret = Parser::new(&allocator, &source_text, source_type).parse();

    if ret.errors.is_empty() {
        println!("{}", serde_json::to_string_pretty(&ret.program).unwrap());
        println!("Parsed Successfully.");
    } else {
        for error in ret.errors {
            let error = error.with_source_code(source_text.clone());
            println!("{error:?}");
        }
    }
}
