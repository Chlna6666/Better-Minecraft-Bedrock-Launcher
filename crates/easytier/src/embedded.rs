use crate::{
    common::config::{Flags, gen_default_flags},
    proto::common::CompressionAlgoPb,
};

/// EasyTier 作为库嵌入应用时使用的网络模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbeddedNetworkMode {
    /// 创建系统 TUN 设备，通过虚拟网卡访问 EasyTier 网络。
    Tun,
    /// 不创建 TUN 设备，通过 SOCKS5/端口转发访问 EasyTier 网络。
    NoTun,
}

#[derive(Debug, thiserror::Error)]
pub enum EmbeddedProfileError {
    #[error("TUN mode requires the `tun` feature")]
    TunFeatureDisabled,
    #[error("no_tun mode requires the `socks5` feature (which enables smoltcp)")]
    NoTunFeatureDisabled,
    #[error("embedded game profile requires the `aes-gcm` or `wireguard` feature")]
    EncryptionFeatureDisabled,
}

/// 生成适合低延迟游戏 P2P/中转场景的 EasyTier Flags。
///
/// 此配置保持 P2P 穿透、TCP/UDP 监听和可选 Zstd 压缩，同时支持：
/// - `Tun`：通过系统虚拟网卡通信；
/// - `NoTun`：不创建虚拟网卡，通过端口转发通信。
///
/// 推荐嵌入依赖：
/// `default-features = false, features = ["tun", "socks5", "aes-gcm", "zstd"]`。
pub fn game_network_flags(mode: EmbeddedNetworkMode) -> Result<Flags, EmbeddedProfileError> {
    match mode {
        EmbeddedNetworkMode::Tun if !cfg!(feature = "tun") => {
            return Err(EmbeddedProfileError::TunFeatureDisabled);
        }
        EmbeddedNetworkMode::NoTun if !cfg!(feature = "socks5") => {
            return Err(EmbeddedProfileError::NoTunFeatureDisabled);
        }
        _ => {}
    }

    if !cfg!(any(feature = "aes-gcm", feature = "wireguard")) {
        return Err(EmbeddedProfileError::EncryptionFeatureDisabled);
    }

    let mut flags = gen_default_flags();
    flags.bind_device = false;
    flags.no_tun = matches!(mode, EmbeddedNetworkMode::NoTun);
    flags.use_smoltcp = false;
    flags.disable_p2p = false;
    flags.encryption_algorithm = "aes-gcm".to_string();

    #[cfg(feature = "zstd")]
    {
        flags.data_compress_algo = CompressionAlgoPb::Zstd.into();
    }

    #[cfg(not(feature = "zstd"))]
    {
        flags.data_compress_algo = CompressionAlgoPb::None.into();
    }

    Ok(flags)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_keeps_p2p_enabled() {
        if cfg!(feature = "tun") && cfg!(any(feature = "aes-gcm", feature = "wireguard")) {
            let flags = game_network_flags(EmbeddedNetworkMode::Tun).unwrap();
            assert!(!flags.disable_p2p);
            assert!(!flags.no_tun);
        }
    }

    #[test]
    fn no_tun_requires_socks5() {
        if !cfg!(feature = "socks5") {
            assert!(matches!(
                game_network_flags(EmbeddedNetworkMode::NoTun),
                Err(EmbeddedProfileError::NoTunFeatureDisabled)
            ));
        }
    }
}
