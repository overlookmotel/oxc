use crate::{Atom, INLINE_FLAG, MAX_LEN_INLINE};

const BASE54_CHARS: &[u8; 64] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ$_0123456789";

const MAX_MANGLED_LEN: usize = match std::mem::size_of::<usize>() {
    // `base54(u64::MAX as usize)` returns 'ZrN6rN6rN6r' (11 bytes)
    8 => 11,
    // `base54(u32::MAX as usize)` returns 'vUdzUd' (6 bytes)
    4 => 6,
    _ => panic!("Unsupported pointer width"),
};
const _: () = assert!(MAX_MANGLED_LEN <= MAX_LEN_INLINE);

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

impl Atom<'static> {
    /// Get the shortest mangled name for a given n.
    /// Code adapted from [terser](https://github.com/terser/terser/blob/8b966d687395ab493d2c6286cc9dd38650324c11/lib/scope.js#L1041-L1051).
    // TODO: This is meant to be a faster version, but benchmark it to see if it actually is.
    // I don't even know if this is a hot path, so whether it's worthwhile optimizing.
    #[doc(hidden)]
    pub fn base54_faster(n: usize) -> Self {
        let mut num = n;
        let mut bytes = [0u8; MAX_LEN_INLINE];

        // Base 54 at first because these are the usable first characters in JavaScript identifiers
        // <https://tc39.es/ecma262/#prod-IdentifierStart>
        let base = 54usize;
        bytes[0] = (num % base) as u8;
        num /= base;

        // Base 64 for the rest because after the first character we can also use 0-9 too
        // <https://tc39.es/ecma262/#prod-IdentifierPart>
        let mut len = 1;
        while num > 0 {
            num -= 1;
            unsafe {
                *bytes.get_unchecked_mut(len) = (num & 63) as u8;
            }
            num >>= 6; // num /= 64
            len += 1;
        }

        // Separate loop with static iteration count to create a single SIMD instruction
        // to look up all chars in one go
        for i in 0..MAX_LEN_INLINE {
            // SAFETY: We know `bytes[i]` is always in bounds for the bytes we filled in,
            // and initial values are 0, so they're in bounds too
            bytes[i] = unsafe { *BASE54_CHARS.get_unchecked(bytes[i] as usize) };
        }

        // String can't fill whole buffer, so no need to check if over-writing last byte
        const LAST_BYTE_INDEX: usize = MAX_LEN_INLINE - 1;
        bytes[LAST_BYTE_INDEX] = len as u8 | INLINE_FLAG;

        // SAFETY: We've created a [u8; MAX_LEN_INLINE] which represents a valid inline `Atom`
        unsafe { std::mem::transmute(bytes) }
    }
}
