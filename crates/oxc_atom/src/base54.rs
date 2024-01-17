use crate::{Atom, INLINE_FLAG, MAX_LEN_INLINE};
use oxc_index::const_assert;

const BASE54_CHARS: &[u8; 64] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ$_0123456789";

const MAX_MANGLED_LEN: usize = match std::mem::size_of::<usize>() {
    // `base54(u64::MAX as usize)` returns 'ZrN6rN6rN6r' (11 bytes)
    8 => 11,
    // `base54(u32::MAX as usize)` returns 'vUdzUd' (6 bytes)
    4 => 6,
    _ => panic!("Unsupported pointer width"),
};
const_assert!(MAX_MANGLED_LEN <= MAX_LEN_INLINE);

impl Atom<'static> {
    /// Get the shortest mangled name for a given n.
    /// Code adapted from [terser](https://github.com/terser/terser/blob/8b966d687395ab493d2c6286cc9dd38650324c11/lib/scope.js#L1041-L1051).
    pub fn base54(n: usize) -> Self {
        let mut num = n;
        let mut ret = [0u8; MAX_MANGLED_LEN];

        // Base 54 at first because these are the usable first characters in JavaScript identifiers
        // <https://tc39.es/ecma262/#prod-IdentifierStart>
        let base = 54usize;
        ret[0] = BASE54_CHARS[num % base];
        num /= base;
        // Base 64 for the rest because after the first character we can also use 0-9 too
        // <https://tc39.es/ecma262/#prod-IdentifierPart>
        let mut len = 1;
        let base = 64usize;
        while num > 0 {
            num -= 1;
            ret[len] = BASE54_CHARS[num % base];
            num /= base;
            len += 1;
        }

        unsafe {
            // SAFETY: Always valid UTF8 as only made up of ASCII characters
            let str = std::str::from_utf8_unchecked(&ret[0..len]);

            // SAFETY: No value of `n` can produce a string longer than `MAX_LEN_INLINE`
            Self::new_inline(str)
        }
    }
}

const BASE64_CHARS_AND_NULL: &[u8; 65] =
    b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ$_0123456789\0";
const NULL_INDEX: u8 = BASE64_CHARS_AND_NULL.len() as u8 - 1;

const_assert!(MAX_MANGLED_LEN < MAX_LEN_INLINE);

impl Atom<'static> {
    /// Get the shortest mangled name for a given n.
    /// Code adapted from [terser](https://github.com/terser/terser/blob/8b966d687395ab493d2c6286cc9dd38650324c11/lib/scope.js#L1041-L1051).
    // TODO: Limit `n` to `u32::MAX`. Not feasable to have more than 4 billion bindings in an AST!
    // TODO: This is meant to be a faster version, but benchmark it to see if it actually is.
    // I don't even know if this is a hot path, so whether it's worthwhile optimizing.
    #[doc(hidden)]
    pub fn base54_faster(n: usize) -> Self {
        let bytes = if n < 54 {
            // Fast path for low values of `n`
            let mut bytes = [0u8; MAX_LEN_INLINE];
            bytes[0] = BASE64_CHARS_AND_NULL[n];
            bytes[MAX_LEN_INLINE - 1] = 1 | INLINE_FLAG;
            bytes
        } else {
            // Initial values are index of 0 byte in `BASE64_CHARS_AND_NULL` so any indexes
            // which aren't altered later are translated to 0 when converted to bytes.
            // TODO: If `BASE64_CHARS_AND_NULL` not fitting into a single L1 cache line is slower,
            // could drop `9` from possible characters used. This would be at cost of losing 54 possible
            // 2-byte identifiers, but that only applies if `n` >= 3456.
            let mut indexes = [NULL_INDEX; MAX_LEN_INLINE];
            indexes[0] = (n % 54) as u8;
            let mut num = n / 54 - 1;

            let mut len = 1;
            loop {
                if num < 64 {
                    // SAFETY: It's not possible for `len` to reach `MAX_LEN_INLINE`
                    unsafe { *indexes.get_unchecked_mut(len) = num as u8 };
                    len += 1;
                    break;
                }
                // SAFETY: It's not possible for `len` to reach `MAX_LEN_INLINE`
                unsafe { *indexes.get_unchecked_mut(len) = (num % 64) as u8 };
                num = num / 64 - 1;
                len += 1;
            }

            // Separate loop with static iteration count to create a single SIMD instruction
            // to look up all chars in one go
            let mut bytes = [0u8; MAX_LEN_INLINE];
            for i in 0..MAX_LEN_INLINE {
                // SAFETY: We know `indexes[i]` is always in bounds for the values we filled in,
                // and initial values are 64, so they're in bounds too
                bytes[i] = unsafe { *BASE64_CHARS_AND_NULL.get_unchecked(indexes[i] as usize) };
            }

            // String can't fill whole buffer, so no need to check if over-writing last byte
            bytes[MAX_LEN_INLINE - 1] = len as u8 | INLINE_FLAG;
            bytes
        };

        // SAFETY: We've created a [u8; MAX_LEN_INLINE] which represents a valid inline `Atom`
        unsafe { std::mem::transmute(bytes) }
    }
}
