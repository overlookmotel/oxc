//! [`Atom`] is a "small string" implementation, for immutable strings.
//!
//! It stores a string, either:
//!
//! 1. Inline.
//! 2. As reference to string data stored elsewhere.
//! 3. In an arena.
//!
//! Any string [`MAX_LEN_INLINE`] bytes or less (16 bytes on 64-bit systems) is stored
//! inline in the `Atom`.
//!
//! Strings longer than [`MAX_LEN_INLINE`] bytes can be either:
//!
//! 1. Stored as a reference to the string data stored elsewhere ([`Atom::new`]).
//! 2. Stored in an arena ([`Atom::new_in`]).
//!
//! If existing string data will live as long as the `Atom` needs to, use [`Atom::new`]. Otherwise,
//! use [`Atom::new_in`] to copy the string data into an arena, and it'll live as long as the arena.
//!
//! For static strings (short or long), [`Atom::new_const`] creates the `Atom` at compile time,
//! so should be preferred for e.g. `Atom::new_const("React")`.
//!
//! Cloning `Atom`s is very cheap, involving only a bitwise copy of 16 bytes,
//! no matter how long the string, or where the string data is stored.
//! The clone contains another reference to same string data.
//!
//! Note that there is a size limit on strings `Atom` can store ([`MAX_LEN`]).
//! On 64-bit systems, it's not a practical restriction (roughly 2 exabytes),
//! but on 32-bit systems, it's feasible to exceed (512 MiB).
//! `Atom` will panic if length exceeds this limit, rather than causing UB.
//!
//! # Implementation details
//!
//! <details>
//!
//! ## Overview
//!
//! Implementation is based on [`compact_str`]. Differences are:
//!
//! 1. `Atom` is immutable. An `Atom` cannot be mutated after it's created.
//! 2. `Atom` does not copy string data to heap/arena unless asked to ([`Atom::new_in`]).
//! 3. `Atom` stores strings longer than [`MAX_LEN_INLINE`] bytes as a reference to existing data,
//!    or copies to an arena. `CompactString` always copies string data to general heap.
//! 4. `CompactString` is 24 bytes long, `Atom` is 16 bytes (on 64-bit systems).
//!
//! ## Representations
//!
//! `Atom` is 16 bytes (assuming 64-bit system), and it can store a string of up to 16 bytes
//! length inline. This is possible by utilising an invariant of UTF8 encoding.
//!
//! `Atom` has 2 different representations:
//!
//! #### 1. Inline
//!
//! * String content is stored in the `Atom`, starting at byte 0.
//! * Length + a 3-bit flag 0b111 is stored in last byte.
//! * If string length is 16, the last byte contains the last byte of the string.
//!
//! #### 2. Out of band
//!
//! * First 8 bytes contain a pointer to start of string data, stored elsewhere.
//! * Last 8 bytes contain length of string stored as `usize` in little-endian byte order.
//! * Top 3 bits in last byte contain flag 0b110.
//!
//! ## Distinguishing the 2 representations
//!
//! The discriminant is the top 3 bits of the last byte.
//!
//! #### 1. Inline, 16 bytes length
//!
//! If the string is 16 bytes long, it is stored inline, and entire contents of `Atom` is the string.
//!
//! Properly-formed UTF8 strings end with a last byte in range 0-191 (`0b00000000` - `0b10111111`).
//! i.e. the top 2 bits can only be `0b00`, `0b01` or `0b10`.
//!
//! `Atom` uses bit pattern `0b11` in top 2 bits to represent all other possibilities.
//!
//! Therefore an `Atom` with `0b00`, `0b01` or `0b10` as top 2 bits of the last byte
//! (i.e. byte value less than 192) is a 16-byte string stored inline.
//!
//! #### 2. Inline, less than 16 bytes length
//!
//! If the string is less than 16 bytes long, the last byte is spare.
//!
//! The top 3 bits are set to `0b111`, and remaining 5 lower bits contain the length.
//! Max inline length is 16, which takes 5 bits.
//!
//! Therefore an `Atom` with `0b111` as top 3 bits of last byte is an inline string
//! with length less than 16.
//!
//! #### 3. Out of band
//!
//! If the string is stored out of band, last 8 bytes contain the length in little-endian byte order.
//! So the last byte is the most significant byte of the length.
//!
//! Length is capped at [`MAX_LEN`], which is `std::mem::size_of<usize>() * 8 - 3` bits
//! (i.e. 61 bits on a 64-bit system). Therefore the top 3 bits are unused by the length.
//! They are set to `0b110`.
//!
//! Therefore an `Atom` with `0b110` as top 3 bits of last byte is an out of band string.
//!
//! ## Niche
//!
//! None of these patterns allow for a value of 255 (`0b11111111`) in the last byte.
//! This pattern is considered invalid for `Atom`'s last byte, and the compiler uses that
//! to create a "niche", which contains the discriminant for `Option<Atom>`.
//! So `Option<Atom>` is 16 bytes, same as `Atom`.
//!
//! ## 32-bit systems
//!
//! Explanation above is based on 64-bit systems, where pointers are 8 bytes. All of the above
//! works the same on 32-bit systems, but the cap on maximum length [`MAX_LEN`] is lower -
//! 512 MiB minus 1 byte - which takes 29 bits, leaving the top 3 bits free for the discriminant.
//!
//! </details>
//!
//! [`compact_str`]: https://docs.rs/compact_str

use std::{
    borrow::Borrow,
    cmp::min,
    fmt,
    marker::PhantomData,
    mem::{self, size_of},
    ops::Deref,
    slice, str,
};

use oxc_allocator::Allocator;
use oxc_index::{assert_eq_size, const_assert};

mod base54;
mod heap;
mod inline;
mod nonmax;

use heap::HeapBuffer;
use inline::InlineBuffer;
use nonmax::NonMaxU8;

// Implementation depends on 64-bit or 32-bit pointers
const USIZE_SIZE: usize = size_of::<usize>();
const_assert!(USIZE_SIZE == 8 || USIZE_SIZE == 4);

/// Max length of a string [`Atom`] can store inline.
///
/// 16 bytes on 64-bit systems, 8 bytes on 32-bit systems.
pub const MAX_LEN_INLINE: usize = size_of::<Atom>();
const_assert!(MAX_LEN_INLINE == USIZE_SIZE * 2);

// Flags for heap or inline are contained in top 3 bits of last byte
const FLAG_MASK: u8 = 0b11100000;

/// Max length of a string [`Atom`] can store.
///
/// Max is roughly 2 exabytes on 64-bit systems, or 1 byte less than 512 MiB on 32-bit systems.
/// Reason for this restriction is that length has to be stored in 3 bits less than a `usize`,
/// as top 3 bits are used for flags.
//
// TODO: To release this size restriction for 32-bit systems, could use same trick
// that `CompactStr` does, storing length on the heap.
// However, that would require changing `Atom::new` to take an `Allocator` as parameter,
// because it needs to be able to allocate space to store the length, and implementing `From<&str>`
// would not be possible.
// That'd make the API much worse just to support very long strings on 32-bit systems.
// Or use 2 x usize for length on 32-bit systems (`Atom` becomes 12 bytes, not 8).
// Do we care about 32-bit systems anyway?
pub const MAX_LEN: usize = usize::MAX >> 3;

// Check `MAX_LEN` does not overlap flag bits, and is as large as it can be without overlapping
#[allow(dead_code)]
const FLAG_MASK_USIZE: usize = (FLAG_MASK as usize) << ((USIZE_SIZE - 1) * 8);
const_assert!(MAX_LEN & FLAG_MASK_USIZE == 0);
const_assert!((MAX_LEN + 1) & FLAG_MASK_USIZE != 0);

/// When string is stored inline, length of string is stored last byte, offset by `INLINE_FLAG`
/// i.e. 0b111xxxxx where xxxxx is the length of string.
/// Inline length is max 16 on 64-bit systems (8 on 32-bit systems) so `length | INLINE_FLAG`
/// cannot be 255 (that would require length of 31). 255 is reserved as niche value.
pub(crate) const INLINE_FLAG: u8 = FLAG_MASK;
const_assert!((MAX_LEN_INLINE | (INLINE_FLAG as usize)) < 255);

/// When string is stored out of line, last byte is 0b110xxxxx,
/// where xxxxx is top 5 bits of string length.
/// The 0 in 3rd bit of flag ensures last byte cannot be 255, which is reserved as niche value.
const HEAP_FLAG: u8 = 0b11000000;
pub(crate) const HEAP_FLAG_USIZE: usize = (HEAP_FLAG as usize) << ((USIZE_SIZE - 1) * 8);

// Check assumptions about pointer sizes
assert_eq_size!(*const (), usize);
assert_eq_size!(*const u8, usize);

// TODO: Implement `Hash`
// TODO: Implement `serde::Serialize`
// TODO: Implement `Send` + `Sync`?

/// See main crate documentation.
//
// NB: `Clone` is a bitwise copy of the *inline* data only.
// Cloning will not make a copy of heap string data, just create another reference to same data.
// This is fine because `Atom`s are immutable.
// Could also derive `Copy` if that's desireable.
//
// TODO: Is the lifetime of the clone same as the original? If not, this doesn't work.
// TODO: Check ASM generated for cloning is just a simple copy of 16 bytes.
// TODO: Check string data cannot be dropped before cloned `Atom`.
#[derive(Clone)]
#[repr(C)]
pub struct Atom<'alloc> {
    // We have a pointer in the representation to properly carry provenance.
    // The comment above is copied from `compact_str`'s implementation.
    // I (@overlookmotel) have no idea what this means, but following their lead!
    // TODO: Why isn't it `*const u8`?
    ptr: *const (),
    // Then we need a `usize` (aka WORD) of data, which we breakup into multiple pieces...
    #[cfg(target_pointer_width = "64")]
    _part1: u32,
    _part2: u16,
    _part3: u8,
    // ...so that the last byte can be a NonMax, which allows the compiler to see a niche value
    last_byte: NonMaxU8,
    // Marker to hold the 'alloc lifetime
    // TODO: Should this be `PhantomData<&'alloc ()>`? Or something else?
    _marker: PhantomData<&'alloc str>,
}

impl<'alloc> Atom<'alloc> {
    /// Create new [`Atom`] from string slice.
    ///
    /// `Atom` will have same lifetime as input `&str`.
    ///
    /// * Short strings ([`MAX_LEN_INLINE`] bytes or less) are stored inline in the `Atom`.
    /// * Longer strings are stored as a reference to the input string's data, with no copying.
    ///
    /// `new` takes a reference, so original string data cannot be mutated until the `Atom` is dropped.
    ///
    /// For static strings, [`Atom::new_const`] is faster, as will be created at compile-time.
    ///
    /// If input string lives less long than `Atom` is required to, use [`Atom::new_in`] instead.
    ///
    /// # Panic
    /// Panics if length of string exceeds [`MAX_LEN`].
    ///
    /// # Examples
    /// ```
    /// use oxc_atom::Atom;
    ///
    /// // Short string stored inline
    /// let short_str: String = "Hello!".into();
    /// let short_atom = Atom::new(&short_str);
    /// assert_eq!(short_atom, "Hello!");
    /// // `short_str.push("?")` here would fail to compile
    ///
    /// // Longer string stored as reference to original string data
    /// let long_str: String = "Well hello there, cowboy...".into();
    /// let long_atom = Atom::new(&long_str);
    /// assert_eq!(long_atom, "Well hello there, cowboy...");
    /// // `long_str.push('?')` here would fail to compile
    /// ```
    //
    // TODO: Test that original string data cannot be mutated
    // TODO: Would it be better to implement this branchless (like `len` and `to_bytes` do)?
    // TODO: Can we make this `fn new(text: impl AsRef<str>) -> Self`? Does that preserve the lifetime?
    pub fn new(text: &'alloc str) -> Self {
        if text.len() <= MAX_LEN_INLINE {
            // SAFETY: Have ensured length is <= MAX_LEN_INLINE
            let buffer = unsafe { InlineBuffer::new(text) };
            Self::from_inline_buffer(buffer)
        } else {
            let buffer = HeapBuffer::new(text);
            Self::from_heap_buffer(buffer)
        }
    }

    /// Create new [`Atom`] from string slice in an arena.
    ///
    /// `Atom` will have same lifetime as the arena.
    ///
    /// * Short strings ([`MAX_LEN_INLINE`] bytes or less) are stored inline in the `Atom`.
    /// * Longer strings are copied into the arena.
    ///
    /// If string will live longer than the arena, and is immutable,
    /// preferable to use [`Atom::new`], as it avoids copying string into the arena.
    ///
    /// For static strings, [`Atom::new_const`] is faster, as will be created at compile-time.
    ///
    /// # Panic
    /// Panics if length of string exceeds [`MAX_LEN`].
    ///
    /// # Examples
    /// ```
    /// use oxc_atom::Atom;
    /// use oxc_allocator::Allocator;
    ///
    /// let allocator = Allocator::default();
    ///
    /// let atom = {
    ///   let mut str: String = "Hello".into();
    ///   str.push('!');
    ///   let atom = Atom::new_in(&str, &allocator);
    ///   str.push_str(" And goodbye!");
    ///   assert_eq!(&str, "Hello! And goodbye!");
    ///   atom
    /// };
    /// // `str` has been dropped now, but `atom` lives as long as `allocator`.
    /// // Contents of `atom` is string at time of `Atom::new_in()` call.
    /// assert_eq!(atom, "Hello!");
    /// ```
    //
    // TODO: Can we make this `fn new_in(text: impl AsRef<str>, allocator: &'alloc Allocator) -> Self`?
    // Would that result in correct lifetime (`Atom<'alloc>`)?
    pub fn new_in<'any>(text: &'any str, allocator: &'alloc Allocator) -> Self {
        if text.len() <= MAX_LEN_INLINE {
            // SAFETY: Have ensured length is <= MAX_LEN_INLINE
            let buffer = unsafe { InlineBuffer::new(text) };
            Self::from_inline_buffer(buffer)
        } else {
            let str = allocator.alloc_str(text);
            let buffer = HeapBuffer::new(str);
            Self::from_heap_buffer(buffer)
        }
    }

    /// Create a new static [`Atom`] at compile time.
    ///
    /// String must have `'static` lifetime. `Atom` will have `'static` lifetime too.
    ///
    /// # Panic
    /// Panics if length of string exceeds [`MAX_LEN`].
    ///
    /// # Examples
    /// ```
    /// use oxc_atom::Atom;
    ///
    /// // Stored inline
    /// const SHORT_ATOM: Atom = Atom::new_const("Short");
    /// assert_eq!(SHORT_ATOM, "Short");
    ///
    /// // Stored as reference
    /// const LONG_STR: &'static str = "Long string that can't be stored inline";
    /// const LONG_ATOM: Atom = Atom::new_const(LONG_STR);
    /// assert_eq!(LONG_ATOM, "Long string that can't be stored inline");
    /// ```
    #[inline]
    pub const fn new_const(text: &'static str) -> Self {
        if text.len() <= MAX_LEN_INLINE {
            let buffer = InlineBuffer::new_const(text);
            Self::from_inline_buffer(buffer)
        } else {
            let buffer = HeapBuffer::new(text);
            Self::from_heap_buffer(buffer)
        }
    }

    /// Create an `Atom` with longer string, stored on heap.
    ///
    /// `Atom` will have same lifetime as input `&str`.
    ///
    /// Stored as a reference to the input string's data, with no copying.
    ///
    /// Usually preferable to use [`Atom::new`]. This method just avoids a length check and branch,
    /// and can be used as a micro-optimization if you know for sure the string is longer than
    /// [`MAX_LEN_INLINE`]. NB: Value of [`MAX_LEN_INLINE`] depends on processor architecture.
    ///
    /// For static strings, [`Atom::new_const`] is faster, as will be created at compile-time.
    ///
    /// # Panic
    /// Panics if string length exceeds [`MAX_LEN`] bytes.
    ///
    /// # Safety
    /// * Caller must ensure length of string is greater than [`MAX_LEN_INLINE`].
    //
    // TODO: Make this method public or delete it.
    #[allow(dead_code)]
    #[inline]
    unsafe fn new_heap(text: &str) -> Self {
        // SAFETY: Safety invariant that `text.len() > MAX_LEN_INLINE` is not actually required.
        // It could lead to incorrect functioning of equality comparisons, but would not be UB.
        // But making this function unsafe, to avoid breaking change later if implementation is
        // modified in a way which does make violating this invariant UB.
        let buffer = HeapBuffer::new(text);
        Self::from_heap_buffer(buffer)
    }

    /// Reinterpret an [`InlineBuffer`] into an [`Atom`].
    ///
    /// # Safety
    /// This is safe because [`InlineBuffer`] and [`Atom`] are the same size,
    /// and the last byte of an [`InlineBuffer`] cannot be 0xFF.
    #[inline(always)]
    const fn from_inline_buffer(buffer: InlineBuffer) -> Self {
        // SAFETY: `InlineBuffer` and `Atom` have the same size
        unsafe { mem::transmute(buffer) }
    }

    /// Reinterpret a [`HeapBuffer`] into an [`Atom`].
    ///
    /// # Safety
    /// This is safe because [`HeapBuffer`] and [`Atom`] are the same size,
    /// and the last byte of a [`HeapBuffer`] cannot be 0xFF.
    #[inline(always)]
    const fn from_heap_buffer(buffer: HeapBuffer) -> Self {
        // SAFETY: `HeapBuffer` and `Atom` have the same size
        unsafe { mem::transmute(buffer) }
    }

    /// Return string length.
    #[inline]
    pub fn len(&self) -> usize {
        // Initially has the value of the inline length, conditionally becomes the heap length
        let mut len = self.len_inline();
        let len_ref = &mut len;
        let heap_len = self.len_heap();

        // Discriminant is stored in the last byte and denotes inline vs heap.
        //
        // Note: We should never add an `else` statement here, keeping the conditional simple allows
        // the compiler to optimize this to a conditional-move instead of a branch.
        if self.is_heap() {
            *len_ref = heap_len;
        }

        *len_ref
    }

    /// Return string length where stored inline.
    ///
    /// Caller must ensure string is stored on heap, or value returned will be wrong.
    #[inline(always)]
    fn len_inline(&self) -> usize {
        // If an inline string has `MAX_LEN_INLINE` bytes length, last byte will be < 192,
        // so `.wrapping_sub(INLINE_FLAG)` will wrap, resulting in a value > MAX_LEN_INLINE,
        // and `min()` will bring it down to `MAX_LEN_INLINE`.
        const_assert!(0u8.wrapping_sub(INLINE_FLAG) as usize > MAX_LEN_INLINE);

        min((self.last_byte()).wrapping_sub(INLINE_FLAG) as usize, MAX_LEN_INLINE)
    }

    /// Return string length where stored on heap.
    ///
    /// Caller must ensure string is stored on heap, or value returned will be wrong.
    #[inline(always)]
    fn len_heap(&self) -> usize {
        let ptr = self as *const Self as *const [u8; USIZE_SIZE];
        let heap_len_bytes = unsafe { ptr.offset(1).read() };
        usize::from_le_bytes(heap_len_bytes) & MAX_LEN
    }

    /// Returns `true` if string has a length of zero bytes.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.last_byte() == INLINE_FLAG
    }

    /// Return the string content as a slice of bytes.
    #[inline]
    pub fn as_slice(&self) -> &[u8] {
        // Initially has the value of the inline pointer, conditionally becomes the heap pointer
        let mut ptr = self as *const Self as *const u8;
        let heap_ptr = self.ptr as *const u8;

        // Initially has the value of the inline length, conditionally becomes the heap length
        let mut len = self.len_inline();
        let heap_len = self.len_heap();

        // Discriminant is stored in the last byte and denotes inline vs heap.
        //
        // Note: We should never add an `else` statement here, keeping the conditional simple allows
        // the compiler to optimize this to a conditional-move instead of a branch.
        if self.is_heap() {
            ptr = heap_ptr;
            len = heap_len;
        }

        // SAFETY: We know the data is valid, aligned, and part of the same contiguous allocated
        // chunk. It's also valid for the lifetime of self.
        unsafe { slice::from_raw_parts(ptr, len) }
    }

    /// Return string as a `&str`.
    // TODO: Is lifetime of return value correct here? Should return `&'alloc str`?
    #[inline]
    pub fn as_str(&self) -> &str {
        // SAFETY: An `Atom` contains valid UTF-8
        unsafe { str::from_utf8_unchecked(self.as_slice()) }
    }

    /// Return raw pointer to string content.
    #[inline]
    pub fn as_ptr(&self) -> *const u8 {
        // Initially has the value of the inline pointer, conditionally becomes the heap pointer
        let mut ptr = self as *const Self as *const u8;
        let heap_ptr = self.ptr as *const u8;

        // Discriminant is stored in the last byte and denotes inline vs heap.
        //
        // Note: We should never add an `else` statement here, keeping the conditional simple allows
        // the compiler to optimize this to a conditional-move instead of a branch.
        if self.is_heap() {
            ptr = heap_ptr;
        }

        ptr
    }

    /// Return if string is stored inline.
    #[inline(always)]
    pub const fn is_inline(&self) -> bool {
        !self.is_heap()
    }

    /// Return if string is stored out of line.
    #[inline(always)]
    pub const fn is_heap(&self) -> bool {
        self.last_byte() & FLAG_MASK == HEAP_FLAG
    }

    /// Get last byte of `Atom`.
    ///
    /// Last byte contains:
    /// * `length | INLINE_FLAG` for inline strings less than `MAX_LEN_INLINE` length.
    /// * Final byte of string for inline strings of exactly `MAX_LEN_INLINE` length.
    /// * `<top 5 bits of length> | HEAP_FLAG` for heap-stored strings.
    #[inline(always)]
    const fn last_byte(&self) -> u8 {
        self.last_byte as u8
    }
}

impl Atom<'static> {
    /// Create an [`Atom`] with short string, stored inline.
    ///
    /// `Atom` will have `'static` lifetime.
    ///
    /// String data is stored inline.
    ///
    /// Usually preferable to use [`Atom::new`], unless you need an `Atom` with `'static` lifetime.
    /// This method just avoids a length check and branch, and can be used as a micro-optimization
    /// if you know for sure the string's length is less than or equal to [`MAX_LEN_INLINE`] bytes.
    ///
    /// NB: Value of [`MAX_LEN_INLINE`] depends on processor architecture.
    ///
    /// For static strings, [`Atom::new_const`] is faster, as will be created at compile-time.
    ///
    /// # Safety:
    /// Caller must ensure length of string is less than or equal to [`MAX_LEN_INLINE`].
    //
    // TODO: Currently unused. Delete this method or make it public.
    // TODO: Does 'any lifetime allow providing a string with a shorter lifetime?
    // e.g. `let atom = { let s = "abc".to_string(); Atom::new_inline(&s, alloc) };`
    // I don't think it works. Check how Bumpalo does this.
    #[inline]
    unsafe fn new_inline<'any>(text: &'any str) -> Self {
        // SAFETY: Caller must ensure length is <= MAX_LEN_INLINE
        let buffer = unsafe { InlineBuffer::new(text) };
        Self::from_inline_buffer(buffer)
    }
}

impl Default for Atom<'static> {
    #[inline(always)]
    fn default() -> Self {
        Atom::new_const("")
    }
}

impl<'alloc> Deref for Atom<'alloc> {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

impl<'alloc> AsRef<str> for Atom<'alloc> {
    #[inline]
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

// TODO: Are these lifetimes correct?
// Any string whose lifetime exceeds lifetime of the `Atom` is OK.
impl<'alloc> From<&'alloc str> for Atom<'alloc> {
    /// # Panic
    /// Panics if length of string exceeds [`MAX_LEN`].
    fn from(s: &'alloc str) -> Self {
        Self::new(s)
    }
}

impl<'alloc> Borrow<str> for Atom<'alloc> {
    #[inline]
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

/*
// TODO: Implement this + also `From<oxc_allocator::String>`
// Should re-use the String's allocation not copy.
impl From<String> for Atom {
    /// # Panic
    /// Panics if length of string exceeds [`MAX_LEN`].
    fn from(s: String) -> Self {
        Self(s.into())
    }
}

// TODO: Implement this
// Should re-use the Cow's allocation not copy, as long as the Cow outlives 'alloc
impl From<Cow<'_, str>> for Atom {
    /// # Panic
    /// Panics if length of string exceeds [`MAX_LEN`].
    fn from(s: Cow<'_, str>) -> Self {
        Self(s.into())
    }
}
*/

impl<'alloc, T: AsRef<str>> PartialEq<T> for Atom<'alloc> {
    fn eq(&self, other: &T) -> bool {
        self.as_str() == other.as_ref()
    }
}

impl<'alloc> PartialEq<Atom<'alloc>> for &str {
    fn eq(&self, other: &Atom) -> bool {
        *self == other.as_str()
    }
}

impl<'alloc> Eq for Atom<'alloc> {}

/*
// TODO: How to prevent conflicting implementation error with implementation for `AsRef<str>` above?
impl<'alloc> PartialEq for Atom<'alloc> {
    // TODO: Is this implementation faster than simply `self.as_str() == other.as_str()`?
    // Intention is to avoid memory access to get the string contents unless comparison cannot
    // be achieved looking at the inline bytes alone.
    // But maybe the branching is slower than a memory access.
    // `compact_str` just uses `self.as_str() == other.as_str()`.
    fn eq(&self, other: &Self) -> bool {
        let [self_top, self_bottom] = self.as_two_usizes();
        let [other_top, other_bottom] = other.as_two_usizes();

        if other_bottom != self_bottom {
            // 2 identical strings always have same lower half.
            // * Short strings are always stored inline, and long strings on heap,
            //   so not possible for same string to have both inline and heap representations.
            //   Bottom bytes contain a flag for inline/heap so if 1 string is inline and 1 is heap,
            //   their bottom bytes differ.
            // * For inline strings, bottom bytes includes length and 2nd half of the string.
            //   If they're different, the strings are different.
            // * For heap strings, bottom bytes contain length.
            //   If they're different, the strings are different.
            // All the above cases are covered by this check.
            false
        } else {
            // We now know that either both strings are heap, or both are inline
            if self.is_inline() {
                // Already compared the bottom bytes, so now compare the top bytes
                other_top == self_top
            } else {
                // Both strings are stored on heap and have same length.
                // Need to compare the string content.
                // SAFETY: We already established both strings are stored on heap.
                unsafe { other.heap_str() == self.heap_str() }
            }
        }
    }
}

impl<'alloc> Atom<'alloc> {
    /// Get string content for a heap string.
    ///
    /// # Safety
    /// Caller must ensure `Atom` is stored on heap before calling this.
    #[inline]
    unsafe fn heap_str(&self) -> &str {
        let ptr = self.ptr as *const u8;
        let len = self.len_heap();

        // SAFETY: Caller must ensure this is a heap-stored Atom.
        // If they do, we know the data is valid, aligned, and part of the same contiguous allocated
        // chunk. It's also valid for the lifetime of self.
        let bytes = slice::from_raw_parts(ptr, len);
        str::from_utf8_unchecked(bytes)
    }

    /// Get `Atom` as `&[usize; 2]`.
    #[inline(always)]
    const fn as_two_usizes(&self) -> &[usize; 2] {
        // SAFETY: `Atom` and `[usize; 2]` have same size
        unsafe { mem::transmute(self) }
    }
}
*/

impl<'alloc> fmt::Debug for Atom<'alloc> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_str(), f)
    }
}

impl<'alloc> fmt::Display for Atom<'alloc> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self.as_str(), f)
    }
}

#[cfg(test)]
mod tests {
    use super::Atom;
    use oxc_allocator::Allocator;

    #[test]
    fn new_empty() {
        let atom = Atom::new("");
        assert!(atom.is_empty());
        assert_eq!(atom.len(), 0);
        assert!(atom.is_inline());
        assert!(!atom.is_heap());
        // TODO: Check how `CompactString`'s tests test for equality.
        // Would `assert_eq!(&*atom, "")` be better?
        assert_eq!(atom, "");
    }

    #[test]
    fn new_inline() {
        let atom = Atom::new("a");
        assert!(!atom.is_empty());
        assert_eq!(atom.len(), 1);
        assert!(atom.is_inline());
        assert!(!atom.is_heap());
        assert_eq!(atom, "a");

        let atom = Atom::new("abcdefgh");
        assert!(!atom.is_empty());
        assert_eq!(atom.len(), 8);
        assert!(atom.is_inline());
        assert!(!atom.is_heap());
        assert_eq!(atom, "abcdefgh");

        #[cfg(target_pointer_width = "64")]
        {
            let atom = Atom::new("abcdefghijklmnop");
            assert!(!atom.is_empty());
            assert_eq!(atom.len(), 16);
            assert!(atom.is_inline());
            assert!(!atom.is_heap());
            assert_eq!(atom, "abcdefghijklmnop");
        }
    }

    #[test]
    fn new_heap() {
        #[cfg(target_pointer_width = "32")]
        {
            let atom = Atom::new("abcdefghi");
            assert!(!atom.is_empty());
            assert_eq!(atom.len(), 9);
            assert!(atom.is_heap());
            assert!(!atom.is_inline());
            assert_eq!(atom, "abcdefghi");
        }

        let atom = Atom::new("abcdefghijklmnopq");
        assert!(!atom.is_empty());
        assert_eq!(atom.len(), 17);
        assert!(atom.is_heap());
        assert!(!atom.is_inline());
        assert_eq!(atom, "abcdefghijklmnopq");

        let str = "x".repeat(256 * 1024);
        let atom = Atom::new(&str);
        assert!(!atom.is_empty());
        assert_eq!(atom.len(), 256 * 1024);
        assert!(atom.is_heap());
        assert!(!atom.is_inline());
        assert_eq!(&*atom, &*str);
    }

    #[test]
    fn new_in_empty() {
        let alloc = Allocator::default();

        let atom = {
            let mut str = String::default();
            let atom = Atom::new_in(&str, &alloc);
            str.push('a');
            atom
        };
        assert!(atom.is_empty());
        assert_eq!(atom.len(), 0);
        assert!(atom.is_inline());
        assert!(!atom.is_heap());
        // TODO: Check how `CompactString`'s tests test for equality.
        // Would `assert_eq!(&*atom, "")` be better?
        assert_eq!(atom, "");
    }

    #[test]
    fn new_in_inline() {
        let alloc = Allocator::default();

        let atom = {
            let mut str = "a".to_string();
            let atom = Atom::new_in(&str, &alloc);
            str.push('b');
            atom
        };
        assert!(!atom.is_empty());
        assert_eq!(atom.len(), 1);
        assert!(atom.is_inline());
        assert!(!atom.is_heap());
        assert_eq!(atom, "a");

        let atom = {
            let mut str = "abcdefgh".to_string();
            let atom = Atom::new_in(&str, &alloc);
            str.push('i');
            atom
        };
        assert!(!atom.is_empty());
        assert_eq!(atom.len(), 8);
        assert!(atom.is_inline());
        assert!(!atom.is_heap());
        assert_eq!(atom, "abcdefgh");

        #[cfg(target_pointer_width = "64")]
        {
            let atom = {
                let mut str = "abcdefghijklmnop".to_string();
                let atom = Atom::new_in(&str, &alloc);
                str.push('q');
                atom
            };
            assert!(!atom.is_empty());
            assert_eq!(atom.len(), 16);
            assert!(atom.is_inline());
            assert!(!atom.is_heap());
            assert_eq!(atom, "abcdefghijklmnop");
        }
    }

    #[test]
    fn new_in_heap() {
        let alloc = Allocator::default();

        #[cfg(target_pointer_width = "32")]
        {
            let atom = {
                let mut str = "abcdefghi".to_string();
                let atom = Atom::new_in(&str, &alloc);
                str.push('j');
                atom
            };
            assert!(!atom.is_empty());
            assert_eq!(atom.len(), 9);
            assert!(atom.is_heap());
            assert!(!atom.is_inline());
            assert_eq!(atom, "abcdefghi");
        }

        let atom = {
            let mut str = "abcdefghijklmnopq".to_string();
            let atom = Atom::new_in(&str, &alloc);
            str.push('q');
            atom
        };
        assert!(!atom.is_empty());
        assert_eq!(atom.len(), 17);
        assert!(atom.is_heap());
        assert!(!atom.is_inline());
        assert_eq!(atom, "abcdefghijklmnopq");

        let atom = {
            let mut str = "x".repeat(256 * 1024);
            let atom = Atom::new_in(&str, &alloc);
            str.push('x');
            atom
        };
        assert!(!atom.is_empty());
        assert_eq!(atom.len(), 256 * 1024);
        assert!(atom.is_heap());
        assert!(!atom.is_inline());
        assert_eq!(&*atom, &*"x".repeat(256 * 1024));
    }

    #[test]
    fn new_const_empty() {
        const ATOM: Atom = Atom::new_const("");
        assert!(ATOM.is_empty());
        assert_eq!(ATOM.len(), 0);
        assert!(ATOM.is_inline());
        assert!(!ATOM.is_heap());
        assert_eq!(ATOM, "");
    }

    #[test]
    fn new_const_inline() {
        const ATOM_1: Atom = Atom::new_const("a");
        assert!(!ATOM_1.is_empty());
        assert_eq!(ATOM_1.len(), 1);
        assert!(ATOM_1.is_inline());
        assert!(!ATOM_1.is_heap());
        assert_eq!(ATOM_1, "a");

        const ATOM_8: Atom = Atom::new_const("abcdefgh");
        assert!(!ATOM_8.is_empty());
        assert_eq!(ATOM_8.len(), 8);
        assert!(ATOM_8.is_inline());
        assert!(!ATOM_8.is_heap());
        assert_eq!(ATOM_8, "abcdefgh");

        #[cfg(target_pointer_width = "64")]
        {
            const ATOM_16: Atom = Atom::new_const("abcdefghijklmnop");
            assert!(!ATOM_16.is_empty());
            assert_eq!(ATOM_16.len(), 16);
            assert!(ATOM_16.is_inline());
            assert!(!ATOM_16.is_heap());
            assert_eq!(ATOM_16, "abcdefghijklmnop");
        }
    }

    #[test]
    fn new_const_heap() {
        #[cfg(target_pointer_width = "32")]
        {
            const ATOM_9: Atom = Atom::new_const("abcdefghi");
            assert!(!ATOM_9.is_empty());
            assert_eq!(ATOM_9.len(), 9);
            assert!(ATOM_9.is_heap());
            assert!(!ATOM_9.is_inline());
            assert_eq!(ATOM_9, "abcdefghi");
        }

        const ATOM_17: Atom = Atom::new_const("abcdefghijklmnopq");
        assert!(!ATOM_17.is_empty());
        assert_eq!(ATOM_17.len(), 17);
        assert!(ATOM_17.is_heap());
        assert!(!ATOM_17.is_inline());
        assert_eq!(ATOM_17, "abcdefghijklmnopq");

        const LONG_STR: &'static str = "0abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMONPQRSTUVWXYZ0123456789_1abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMONPQRSTUVWXYZ0123456789_2abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMONPQRSTUVWXYZ0123456789_3abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMONPQRSTUVWXYZ0123456789_4abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMONPQRSTUVWXYZ0123456789_5abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMONPQRSTUVWXYZ0123456789_6abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMONPQRSTUVWXYZ0123456789_7abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMONPQRSTUVWXYZ0123456789_8abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMONPQRSTUVWXYZ0123456789_9abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMONPQRSTUVWXYZ0123456789_AabcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMONPQRSTUVWXYZ0123456789_BabcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMONPQRSTUVWXYZ0123456789_CabcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMONPQRSTUVWXYZ0123456789_DabcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMONPQRSTUVWXYZ0123456789_EabcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMONPQRSTUVWXYZ0123456789_FabcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMONPQRSTUVWXYZ0123456789_";
        const ATOM_1024: Atom = Atom::new_const(LONG_STR);
        assert!(!ATOM_1024.is_empty());
        assert_eq!(ATOM_1024.len(), 1024);
        assert!(ATOM_1024.is_heap());
        assert!(!ATOM_1024.is_inline());
        assert_eq!(ATOM_1024, LONG_STR);
    }
}

// TODO: Tests for non-ASCII strings
// TODO: Test for panic if try to create `Atom` from string longer than `MAX_LEN`
