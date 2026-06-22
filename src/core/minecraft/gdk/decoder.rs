use crate::core::minecraft::gdk::key::{CikKey, KeySignal};

#[derive(Clone)]
pub struct MsiXVDDecoder {
    d: KeySignal,
    t: KeySignal,
}

impl MsiXVDDecoder {
    pub fn new(key: &CikKey) -> Result<Self, String> {
        Ok(Self {
            d: KeySignal::new(&key.d_key)?,
            t: KeySignal::new(&key.t_key)?,
        })
    }

    fn gf128_mul(tweak: &mut [u8; 16]) {
        let mut low_bytes = [0u8; 8];
        let mut high_bytes = [0u8; 8];
        low_bytes.copy_from_slice(&tweak[..8]);
        high_bytes.copy_from_slice(&tweak[8..16]);

        let low = u64::from_le_bytes(low_bytes);
        let high = u64::from_le_bytes(high_bytes);
        let carry_low = (low >> 63) != 0;
        let carry_high = (high >> 63) != 0;

        let mut next_low = low << 1;
        let mut next_high = high << 1;

        if carry_high {
            next_low ^= 0x87;
        }

        if carry_low {
            next_high |= 1;
        }

        tweak[..8].copy_from_slice(&next_low.to_le_bytes());
        tweak[8..16].copy_from_slice(&next_high.to_le_bytes());
    }

    fn decrypt_block(&self, input: &[u8], output: &mut [u8], tweak: &[u8; 16]) {
        let mut block = [0u8; 16];
        block.copy_from_slice(&input[..16]);
        xor_block(&mut block, tweak);
        self.d.decrypt_block(&mut block);
        xor_block(&mut block, tweak);
        output[..16].copy_from_slice(&block);
    }

    pub fn decrypt(&self, input: &[u8], output: &mut [u8], tweak_iv: &[u8]) -> usize {
        if tweak_iv.len() < 16 {
            return 0;
        }

        let length = input.len().min(output.len());
        if length == 0 {
            return 0;
        }

        let mut remaining_blocks = length >> 4;
        let leftover = length & 0xF;

        if leftover != 0 {
            if remaining_blocks == 0 {
                return 0;
            }
            remaining_blocks -= 1;
        }

        let mut tweak = [0u8; 16];
        tweak.copy_from_slice(&tweak_iv[..16]);
        self.t.encrypt_block(&mut tweak);

        let mut offset = 0;
        for _ in 0..remaining_blocks {
            self.decrypt_block(
                &input[offset..offset + 16],
                &mut output[offset..offset + 16],
                &tweak,
            );
            Self::gf128_mul(&mut tweak);
            offset += 16;
        }

        if leftover != 0 {
            let mut final_tweak = tweak;
            Self::gf128_mul(&mut final_tweak);

            let mut p_n_minus_1_raw = [0u8; 16];
            self.decrypt_block(
                &input[offset..offset + 16],
                &mut p_n_minus_1_raw,
                &final_tweak,
            );

            let partial_offset = offset + 16;
            let mut c_prime_n_minus_1 = p_n_minus_1_raw;
            for j in 0..leftover {
                output[partial_offset + j] = c_prime_n_minus_1[j];
                c_prime_n_minus_1[j] = input[partial_offset + j];
            }

            let mut p_n_minus_1 = c_prime_n_minus_1;
            xor_block(&mut p_n_minus_1, &tweak);
            self.d.decrypt_block(&mut p_n_minus_1);
            xor_block(&mut p_n_minus_1, &tweak);
            output[offset..offset + 16].copy_from_slice(&p_n_minus_1);
        }

        length
    }
}

fn xor_block(block: &mut [u8; 16], tweak: &[u8; 16]) {
    for (block_byte, tweak_byte) in block.iter_mut().zip(tweak) {
        *block_byte ^= *tweak_byte;
    }
}

#[cfg(test)]
mod tests {
    use super::MsiXVDDecoder;

    #[test]
    fn gf128_mul_carries_from_low_lane_to_high_lane() {
        let mut tweak = [0u8; 16];
        tweak[7] = 0x80;

        MsiXVDDecoder::gf128_mul(&mut tweak);

        assert_eq!(tweak[7], 0);
        assert_eq!(tweak[8], 1);
    }

    #[test]
    fn gf128_mul_reduces_high_lane_overflow_into_low_byte() {
        let mut tweak = [0u8; 16];
        tweak[15] = 0x80;

        MsiXVDDecoder::gf128_mul(&mut tweak);

        assert_eq!(tweak[0], 0x87);
        assert_eq!(tweak[15], 0);
    }
}
