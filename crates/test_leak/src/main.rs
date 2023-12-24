use std::path::Path;

extern crate oxc_allocator;
extern crate oxc_parser;
extern crate oxc_span;

use oxc_allocator::Allocator;
use oxc_parser::Parser;
use oxc_span::SourceType;

fn main() {
    parse("x = 'ABCDE';");
    parse("x = 'ABCDEFGHIJKLMNOPQRSTUVWXYZ';");
    parse("x = 'ABCDE\\n';");
    parse("x = 'ABCDEFGHIJKLMNOPQRSTUVWXYZ\\n';");
}

fn parse(source: &str) {
    println!("----------------------------------------");
    println!("Parsing: {}", source);

    let source_text = source.to_string();
    let mut allocator = Allocator::default();
    let source_type = SourceType::from_path(Path::new("/path/to/foo.js")).unwrap();
    {
        let ret = Parser::new(&allocator, &source_text, source_type).parse();
        assert!(ret.errors.is_empty());
    }

    let chunk = allocator.iter_allocated_chunks().nth(0).unwrap();
    let bytes: std::vec::Vec<u8> = chunk.iter().map(|b| unsafe { b.assume_init() }).collect();
    hexdump::hexdump(&bytes);
}
