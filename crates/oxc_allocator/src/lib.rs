use std::ops::Deref;

mod arena;

pub use arena::{Box, String, Vec};
use bumpalo::Bump;
use bumpalo::ChunkIter;

#[derive(Default)]
pub struct Allocator {
    bump: Bump,
}

impl Deref for Allocator {
    type Target = Bump;

    fn deref(&self) -> &Self::Target {
        &self.bump
    }
}

impl Allocator {
    pub fn iter_allocated_chunks(&mut self) -> ChunkIter<'_> {
        self.bump.iter_allocated_chunks()
    }
}
