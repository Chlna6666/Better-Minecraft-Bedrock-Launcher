//! GDK (MSIX-VC) 解包模块
//!
//! 该模块提供了用于解析和提取 GDK 加密打包文件的功能。
//!
//! 主要功能通过 `unpack_gdk` 函数暴露，该函数封装了所有复杂的解析和解密逻辑。
//!
//! 模块结构:
//! - `decoder`: 实现核心的 AES-XTS 解密算法。
//! - `header`: 定义了 MSIX-VC 文件的头部数据结构。
//! - `key`: 包含 CIK 密钥的处理和密钥调度逻辑。
//! - `stream`: 实现了对 GDK 文件流的解析、段（Segment）提取和文件重建。
//! - `structs`: 定义了 GDK 文件格式中用到的各种辅助数据结构。

pub mod decoder;
pub mod header;
pub mod key;
pub mod stream;
pub mod structs;

// 从 stream 模块中公开 unpack_gdk 函数，使其可以直接通过 `gdk::unpack_gdk` 调用。
pub use stream::unpack_gdk;
