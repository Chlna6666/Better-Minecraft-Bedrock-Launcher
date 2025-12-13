use std::arch::x86_64::*;
use crate::core::minecraft::gdk::key::{CikKey, KeySinagl};

#[derive(Clone, Copy)]
pub struct MsiXVDDecoder {
    d: KeySinagl,
    t: KeySinagl,
}

impl MsiXVDDecoder {
    pub fn new(key: &CikKey) -> Self {
        unsafe {
            Self {
                d: KeySinagl::new(&key.d_key, true),
                t: KeySinagl::new(&key.t_key, false),
            }
        }
    }

    #[inline]
    #[target_feature(enable = "sse2")]
    unsafe fn gf128_mul(iv: __m128i, mask: __m128i) -> __m128i {
        // C#: Sse2.Add(iv.AsUInt64(), iv.AsUInt64()) -> paddq
        let tmp1 = _mm_add_epi64(iv, iv);

        // C#: Sse2.Shuffle(iv.AsInt32(), 0x13)
        // 0x13 = 00_01_00_11 (indices 0, 1, 0, 3) => [3, 1, 0, 0]?
        // _MM_SHUFFLE(z,y,x,w) -> (z<<6)|(y<<4)|(x<<2)|w
        // We want to match C# logic exactly.
        // C# Shuffle(val, 0x13) result:
        // Word 0 = val[3]
        // Word 1 = val[0]
        // Word 2 = val[1]
        // Word 3 = val[0]
        // Wait, C# uses Little Endian element indexing usually?
        // Let's stick to the instruction: pshufd xmm, xmm, 0x13
        let mut tmp2 = _mm_shuffle_epi32(iv, 0x13);

        // C#: Sse2.ShiftRightArithmetic(tmp2, 31) -> psrad
        tmp2 = _mm_srai_epi32(tmp2, 31);

        // C#: Sse2.And(mask, tmp2) -> pand
        tmp2 = _mm_and_si128(mask, tmp2);

        // C#: Sse2.Xor(tmp1, tmp2) -> pxor
        _mm_xor_si128(tmp1, tmp2)
    }

    #[target_feature(enable = "sse2", enable = "aes")]
    pub unsafe fn decrypt(&self, input: &[u8], output: &mut [u8], tweak_iv: &[u8]) -> usize {
        if tweak_iv.len() < 16 { return 0; }

        let length = input.len().min(output.len());
        if length == 0 { return 0; }

        let mut remaining_blocks = length >> 4;
        let leftover = length & 0xF;

        if leftover != 0 {
            if remaining_blocks == 0 { return 0; }
            remaining_blocks -= 1;
        }

        let mut in_ptr = input.as_ptr() as *const __m128i;
        let mut out_ptr = output.as_mut_ptr() as *mut __m128i;

        // C#: Vector128.Create(0x87, 1) -> [0x87, 0... | 1, 0...]
        // Rust _mm_set_epi8 is High-to-Low.
        // So e15..e8 (High) = 1, e7..e0 (Low) = 0x87
        let mask = _mm_set_epi8(
            0, 0, 0, 0, 0, 0, 0, 1,   // High u64 = 1
            0, 0, 0, 0, 0, 0, 0, 0x87u8 as i8 // Low u64 = 0x87
        );

        let iv_vec = _mm_loadu_si128(tweak_iv.as_ptr() as *const __m128i);
        let mut tweak = self.t.encrypt_unrolled(iv_vec);

        let mut i = 0;
        // 8-way unrolled loop
        while i + 8 <= remaining_blocks {
            // Calculate Tweaks T0..T7
            let t0 = tweak;
            let t1 = Self::gf128_mul(t0, mask);
            let t2 = Self::gf128_mul(t1, mask);
            let t3 = Self::gf128_mul(t2, mask);
            let t4 = Self::gf128_mul(t3, mask);
            let t5 = Self::gf128_mul(t4, mask);
            let t6 = Self::gf128_mul(t5, mask);
            let t7 = Self::gf128_mul(t6, mask);

            // Load Blocks
            let b0 = _mm_xor_si128(_mm_loadu_si128(in_ptr.add(0)), t0);
            let b1 = _mm_xor_si128(_mm_loadu_si128(in_ptr.add(1)), t1);
            let b2 = _mm_xor_si128(_mm_loadu_si128(in_ptr.add(2)), t2);
            let b3 = _mm_xor_si128(_mm_loadu_si128(in_ptr.add(3)), t3);
            let b4 = _mm_xor_si128(_mm_loadu_si128(in_ptr.add(4)), t4);
            let b5 = _mm_xor_si128(_mm_loadu_si128(in_ptr.add(5)), t5);
            let b6 = _mm_xor_si128(_mm_loadu_si128(in_ptr.add(6)), t6);
            let b7 = _mm_xor_si128(_mm_loadu_si128(in_ptr.add(7)), t7);

            // Decrypt
            let d0 = self.d.decrypt_block_unrolled(b0);
            let d1 = self.d.decrypt_block_unrolled(b1);
            let d2 = self.d.decrypt_block_unrolled(b2);
            let d3 = self.d.decrypt_block_unrolled(b3);
            let d4 = self.d.decrypt_block_unrolled(b4);
            let d5 = self.d.decrypt_block_unrolled(b5);
            let d6 = self.d.decrypt_block_unrolled(b6);
            let d7 = self.d.decrypt_block_unrolled(b7);

            // Xor Tweak & Store
            _mm_storeu_si128(out_ptr.add(0), _mm_xor_si128(d0, t0));
            _mm_storeu_si128(out_ptr.add(1), _mm_xor_si128(d1, t1));
            _mm_storeu_si128(out_ptr.add(2), _mm_xor_si128(d2, t2));
            _mm_storeu_si128(out_ptr.add(3), _mm_xor_si128(d3, t3));
            _mm_storeu_si128(out_ptr.add(4), _mm_xor_si128(d4, t4));
            _mm_storeu_si128(out_ptr.add(5), _mm_xor_si128(d5, t5));
            _mm_storeu_si128(out_ptr.add(6), _mm_xor_si128(d6, t6));
            _mm_storeu_si128(out_ptr.add(7), _mm_xor_si128(d7, t7));

            tweak = Self::gf128_mul(t7, mask);
            in_ptr = in_ptr.add(8);
            out_ptr = out_ptr.add(8);
            i += 8;
        }

        while i < remaining_blocks {
            let in_block = _mm_loadu_si128(in_ptr);
            let tmp = _mm_xor_si128(in_block, tweak);
            let decrypted = self.d.decrypt_block_unrolled(tmp);
            let out_block = _mm_xor_si128(decrypted, tweak);
            _mm_storeu_si128(out_ptr, out_block);

            tweak = Self::gf128_mul(tweak, mask);
            in_ptr = in_ptr.add(1);
            out_ptr = out_ptr.add(1);
            i += 1;
        }

        // CTS for residual partial block (not used for standard GDK segments, but kept for completeness)
        if leftover != 0 {
            let final_tweak = Self::gf128_mul(tweak, mask); // T_n
            let in_block = _mm_loadu_si128(in_ptr); // C_{n-1}
            let tmp = _mm_xor_si128(in_block, final_tweak);
            let decrypted = self.d.decrypt_block_unrolled(tmp);
            let p_n_minus_1_raw = _mm_xor_si128(decrypted, final_tweak);

            let in_bytes_n = in_ptr.add(1) as *const u8;
            let out_bytes_n_minus_1 = out_ptr as *mut u8;
            let out_bytes_n = out_ptr.add(1) as *mut u8;

            let mut c_prime_n_minus_1_buf = [0u8; 16];
            _mm_storeu_si128(c_prime_n_minus_1_buf.as_mut_ptr() as *mut __m128i, p_n_minus_1_raw);

            for j in 0..leftover {
                let c_n_byte = *in_bytes_n.add(j);
                *out_bytes_n.add(j) = c_prime_n_minus_1_buf[j];
                c_prime_n_minus_1_buf[j] = c_n_byte;
            }

            let c_prime_vec = _mm_loadu_si128(c_prime_n_minus_1_buf.as_ptr() as *const __m128i);
            let tmp2 = _mm_xor_si128(c_prime_vec, tweak); // Tweak is T_{n-1}
            let decrypted2 = self.d.decrypt_block_unrolled(tmp2);
            let p_n_minus_1 = _mm_xor_si128(decrypted2, tweak);

            _mm_storeu_si128(out_ptr, p_n_minus_1);
        }

        length
    }
}