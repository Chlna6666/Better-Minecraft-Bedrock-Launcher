use aes::Aes128;
use aes::cipher::{BlockDecrypt, BlockEncrypt, KeyInit};
use tracing::warn;
use uuid::Uuid;

const UTF8_REPLACEMENT_BYTES: &[u8] = &[0xef, 0xbf, 0xbd];

#[derive(Clone, Copy)]
pub struct CikKey {
    pub guid: Uuid,
    pub t_key: [u8; 16],
    pub d_key: [u8; 16],
}

impl CikKey {
    pub const MAX_SIZE: usize = 0x30;

    pub fn new(cik: &[u8]) -> Result<Self, String> {
        if cik.len() < Self::MAX_SIZE {
            return Err(format!("CIK 密钥长度不足: {}", cik.len()));
        }

        let guid_bytes: [u8; 16] = cik[..0x10]
            .try_into()
            .map_err(|_| "CIK GUID 长度无效".to_string())?;
        let guid = Uuid::from_bytes_le(guid_bytes);
        let t_key = cik[0x10..0x20]
            .try_into()
            .map_err(|_| "CIK T 密钥长度无效".to_string())?;
        let d_key = cik[0x20..0x30]
            .try_into()
            .map_err(|_| "CIK D 密钥长度无效".to_string())?;

        Ok(Self { guid, t_key, d_key })
    }

    pub fn find_and_create(data: &[u8], expected_guid_str: &str) -> Result<Self, String> {
        let expected_guid =
            Uuid::parse_str(expected_guid_str).map_err(|error| format!("无效 GUID: {error}"))?;
        let expected_bytes = expected_guid.to_bytes_le();

        if let Ok(key) = Self::new(data) {
            if key.guid == expected_guid {
                return Ok(key);
            }
        }

        if let Some(start_index) = data.windows(16).position(|window| window == expected_bytes) {
            warn!("自动修正 CIK 偏移: {}", start_index);
            if start_index + Self::MAX_SIZE <= data.len() {
                return Self::new(&data[start_index..start_index + Self::MAX_SIZE]);
            }
        }

        if let Ok(text) = std::str::from_utf8(data) {
            let clean_hex: String = text.chars().filter(char::is_ascii_hexdigit).collect();
            if clean_hex.len() >= 96 {
                if let Ok(key) = Self::from_hex_string(&clean_hex[..96]) {
                    if key.guid == expected_guid {
                        return Ok(key);
                    }
                }
            }
        }

        if data
            .windows(UTF8_REPLACEMENT_BYTES.len())
            .any(|window| window == UTF8_REPLACEMENT_BYTES)
        {
            return Err(
                "CIK 密钥内容包含 UTF-8 替换字节，可能被按文本保存导致二进制损坏".to_string(),
            );
        }

        Err("无法找到匹配的 CIK 密钥".to_string())
    }

    pub fn from_hex_string(hex_string: &str) -> Result<Self, String> {
        let cik = hex::decode(hex_string).map_err(|error| error.to_string())?;
        Self::new(&cik)
    }
}

#[derive(Clone)]
pub struct KeySignal {
    cipher: Aes128,
}

impl KeySignal {
    pub fn new(key_bytes: &[u8]) -> Result<Self, String> {
        let key: [u8; 16] = key_bytes
            .try_into()
            .map_err(|_| format!("AES-128 密钥长度无效: {}", key_bytes.len()))?;

        Ok(Self {
            cipher: Aes128::new(&key.into()),
        })
    }

    pub fn decrypt_block(&self, block: &mut [u8; 16]) {
        self.cipher.decrypt_block(block.into());
    }

    pub fn encrypt_block(&self, block: &mut [u8; 16]) {
        self.cipher.encrypt_block(block.into());
    }
}
