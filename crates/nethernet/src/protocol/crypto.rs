//! 发现层加密：AES-256-ECB + PKCS7，外加 HMAC-SHA256 校验。
//!
//! 密钥由固定应用 ID 派生，全网公开——它只是 vanilla 的混淆层，
//! 不提供任何机密性保证。校验和覆盖的是**明文**（与 go-nethernet 一致）。

use crate::consts::{APPLICATION_ID, CHECKSUM_SIZE};
use crate::error::{NethernetError, Result};
use aes::Aes256;
use aes::cipher::{Block, BlockDecrypt, BlockEncrypt, KeyInit};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use std::sync::LazyLock;

const BLOCK_SIZE: usize = 16;

static KEY: LazyLock<[u8; 32]> = LazyLock::new(|| {
    let mut hasher = Sha256::new();
    hasher.update(APPLICATION_ID.to_le_bytes());
    hasher.finalize().into()
});

/// 预初始化的分组密码：避免每个报文重新做密钥扩展。
static CIPHER: LazyLock<Aes256> =
    LazyLock::new(|| Aes256::new_from_slice(KEY.as_slice()).expect("SHA256 输出长度恒为 32 字节"));

/// 就地加密（追加 PKCS7 填充）。
pub fn encrypt_in_place(data: &mut Vec<u8>) {
    let padding = BLOCK_SIZE - data.len() % BLOCK_SIZE;
    #[allow(clippy::cast_possible_truncation)]
    data.resize(data.len() + padding, padding as u8);
    for chunk in data.chunks_exact_mut(BLOCK_SIZE) {
        CIPHER.encrypt_block(Block::<Aes256>::from_mut_slice(chunk));
    }
}

/// 就地解密并去除 PKCS7 填充。
pub fn decrypt_in_place(data: &mut Vec<u8>) -> Result<()> {
    if data.is_empty() || data.len() % BLOCK_SIZE != 0 {
        return Err(NethernetError::protocol("AES 密文长度非法"));
    }
    for chunk in data.chunks_exact_mut(BLOCK_SIZE) {
        CIPHER.decrypt_block(Block::<Aes256>::from_mut_slice(chunk));
    }
    let padding = usize::from(*data.last().expect("长度已校验非空"));
    if padding == 0 || padding > BLOCK_SIZE || padding > data.len() {
        return Err(NethernetError::protocol("PKCS7 填充长度非法"));
    }
    if data[data.len() - padding..]
        .iter()
        .any(|byte| usize::from(*byte) != padding)
    {
        return Err(NethernetError::protocol("PKCS7 填充内容非法"));
    }
    data.truncate(data.len() - padding);
    Ok(())
}

/// 计算明文的 HMAC-SHA256。
#[must_use]
pub fn checksum(data: &[u8]) -> [u8; CHECKSUM_SIZE] {
    let mut mac =
        <Hmac<Sha256> as Mac>::new_from_slice(KEY.as_slice()).expect("HMAC 接受任意长度密钥");
    mac.update(data);
    mac.finalize().into_bytes().into()
}

/// 恒定时间校验。
pub fn verify_checksum(data: &[u8], expected: &[u8]) -> Result<()> {
    let mut mac =
        <Hmac<Sha256> as Mac>::new_from_slice(KEY.as_slice()).expect("HMAC 接受任意长度密钥");
    mac.update(data);
    mac.verify_slice(expected)
        .map_err(|_| NethernetError::protocol("发现报文校验和不匹配"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_matches_reference_derivation() {
        // go-nethernet: sha256.Sum256(binary.LittleEndian.AppendUint64(nil, 0xdeadbeef))
        let mut hasher = Sha256::new();
        hasher.update(0xdead_beef_u64.to_le_bytes());
        let expected: [u8; 32] = hasher.finalize().into();
        assert_eq!(*KEY, expected);
    }

    #[test]
    fn round_trip_all_padding_lengths() {
        for length in 0..48 {
            let original = vec![0xAB_u8; length];
            let mut buffer = original.clone();
            encrypt_in_place(&mut buffer);
            assert_eq!(buffer.len() % BLOCK_SIZE, 0);
            assert!(buffer.len() > length, "PKCS7 必须至少追加一个字节");
            decrypt_in_place(&mut buffer).unwrap();
            assert_eq!(buffer, original, "长度 {length} 未能往返");
        }
    }

    #[test]
    fn rejects_bad_padding() {
        let mut buffer = vec![0_u8; 16];
        encrypt_in_place(&mut buffer);
        let last = buffer.len() - 1;
        buffer[last] ^= 0xFF;
        assert!(decrypt_in_place(&mut buffer).is_err());
    }

    #[test]
    fn rejects_non_block_multiple() {
        let mut buffer = vec![0_u8; 17];
        assert!(decrypt_in_place(&mut buffer).is_err());
    }

    #[test]
    fn checksum_detects_tampering() {
        let data = b"discovery payload";
        let mac = checksum(data);
        verify_checksum(data, &mac).unwrap();
        verify_checksum(b"discovery payloae", &mac).unwrap_err();
    }
}
