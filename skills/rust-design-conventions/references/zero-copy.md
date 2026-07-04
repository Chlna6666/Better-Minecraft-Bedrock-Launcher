# 零拷贝（Zero-Copy）专题

> 零拷贝是高性能系统的核心技术：让数据在内核空间、用户空间、网络之间流动时**尽量避免复制到用户态缓冲区**。Rust 的所有权系统让零拷贝可以做到**安全**（C/C++ 做不到）。

## 1. 什么是"拷贝"——先理解成本

```
传统 I/O 读取 + 写入（read + write）的数据流：

  磁盘 ─→ [DMA 拷贝] → 内核缓冲区 ─→ [CPU 拷贝] → 用户缓冲区 ─→ [CPU 拷贝] → Socket 缓冲区 ─→ [DMA 拷贝] → 网卡
                                  ↑                            ↑
                                  2 次上下文切换                 2 次上下文切换
                                  2 次 CPU 拷贝 + 2 次 DMA 拷贝 = 4 次拷贝

零拷贝（sendfile）：

  磁盘 ─→ [DMA] → 内核缓冲区 ─→ [CPU 拷贝] → Socket 缓冲区 ─→ [DMA] → 网卡
              （数据完全不进入用户空间）

零拷贝（splice / io_uring）：

  磁盘 ─→ [DMA] → 内核缓冲区 ────────────────────→ [DMA] → 网卡
                    （仅 1 次 DMA，通过管道描述符传递）
```

| 方式 | 上下文切换 | CPU 拷贝 | DMA 拷贝 |
|------|-----------|---------|---------|
| read + write | 4 | 2 | 2 |
| mmap + write | 4 | 1 | 2 |
| sendfile | 2 | 1 | 2 |
| sendfile (SG-DMA) | 2 | 0 | 2 |
| splice | 2 | 0 | 2 |

## 2. 零拷贝的三个层次

| 层次 | 含义 | Rust 中的工具 |
|------|------|--------------|
| **L1: 系统调用级** | 内核不把数据拷到用户态 | `io::copy`, `sendfile`, `splice`, `mmap`, `io_uring` |
| **L2: 数据结构级** | 用户态代码避免 `clone()`/`memcpy` | `&[u8]`, `bytes::Bytes`, `Cow`, 切片而非 owned |
| **L3: 序列化级** | 反序列化时不拷贝到新结构 | `zerocopy`, `bytemuck`, `rkyv`, serde zero-copy |

## 3. L1：系统调用级零拷贝

### 3.1 io::copy（文件到网络）

```rust
use std::io;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::fs::File;
use tokio::net::TcpStream;

// ✓ tokio::io::copy 在 Linux 上自动用 copy_file_range / sendfile
async fn serve_file(path: &str, mut out: TcpStream) -> io::Result<()> {
    let mut file = File::open(path).await?;
    // 数据不进入用户态，内核直接从文件描述符拷到 socket
    io::copy(&mut file, &mut out).await?;
    out.flush().await?;
    Ok(())
}
```

### 3.2 sendfile（裸 syscall）

```rust
// 对于已知是普通文件的场景，直接调用 sendfile 最快
use std::os::unix::io::AsRawFd;

extern "C" {
    fn sendfile(out_fd: i32, in_fd: i32, offset: *mut i64, count: usize) -> isize;
}

fn sendfile_all(out_fd: i32, in_fd: i32, mut offset: i64, count: usize) -> io::Result<()> {
    let mut sent = 0;
    while sent < count {
        let n = unsafe {
            sendfile(out_fd, in_fd, &mut offset, count - sent)
        };
        if n <= 0 { return Err(io::Error::last_os_error()); }
        sent += n as usize;
    }
    Ok(())
}
// 注意：实际项目用 tokio 或 nix crate，不要手写 syscall
```

### 3.3 mmap（内存映射文件）

```rust
use memmap2::Mmap;

// ✓ 大文件只读：mmap 映射，访问时按需调入页，无全量拷贝
fn read_large_file(path: &str) -> io::Result<()> {
    let file = File::open(path)?;
    let mmap = unsafe { Mmap::map(&file)? };
    // mmap[..] 是 &[u8]，直接访问，OS 按需分页
    let header = &mmap[..8];
    parse_header(header);
    Ok(())
}

// 适用：
// - 大文件只读索引（数据库）
// - 进程间共享内存
// - 不适合：频繁随机写（用 Buffer I/O）
```

### 3.4 io_uring（Linux 5.1+，最高效）

```rust
// io_uring 提供真正的异步、零拷贝、批量 I/O
// 用 rio 或 io-uring crate
use rio::Rio;

let ring = Rio::new()?;

// 提交多个 I/O 请求，内核批量处理，无系统调用开销
let mut reads = Vec::new();
for file in files {
    let f = File::open(file)?;
    let buf = vec![0u8; 4096];
    reads.push(ring.read_at(&f, &mut buf, 0));
}
// 等待所有完成
for r in reads { let _ = r.await?; }
```

### 3.5 Vectored I/O（scatter-gather）

```rust
use std::io::{IoSlice, IoSliceMut, Read, Write};

// ✓ 一次系统调用读写多个不连续缓冲区，避免拼接待写数据
let buf1 = [1, 2, 3, 4];
let buf2 = [5, 6, 7, 8];
let bufs = &mut [
    IoSlice::new(&buf1),
    IoSlice::new(&buf2),
];
// write_vectored 一次 syscall 写入两个缓冲区（无需拼接为单个 Vec）
socket.write_vectored(bufs)?;

// Tokio 异步版本
use tokio::io::AsyncWriteExt;
socket.write_vectored(bufs).await?;
```

## 4. L2：数据结构级零拷贝

### 4.1 切片借用是零拷贝的基础

```rust
// ✓ 函数参数用 &[u8] 而非 Vec<u8> / [u8; N]
fn parse_header(data: &[u8]) -> Header {
    // 借用原始 buffer，不拷贝
    Header { magic: u32::from_be_bytes(data[0..4].try_into().unwrap()) }
}

// 调用方拥有数据，被调用方只读借用
let raw = read_socket()?;  // Vec<u8>
let header = parse_header(&raw[..8]);  // 零拷贝借用
// raw 仍存活，header 引用其中字节
```

### 4.2 bytes::Bytes（引用计数缓冲区）

```rust
use bytes::{Bytes, BytesMut, Buf, BufMut};

// Bytes = Arc 包装的字节缓冲区，clone 是 O(1)
let buf: Bytes = Bytes::from_static(b"hello world");
let b1 = buf.clone();  // ✓ O(1)！只增 Arc 计数
let b2 = buf.clone();  // ✓ O(1)
// b1, b2, buf 共享同一块内存

// slice 也是 O(1)，只是调整 offset/len
let hello = buf.slice(0..5);   // "hello"，无拷贝
let world = buf.slice(6..11);  // "world"，无拷贝

// 适用场景：
// - 网络框架（hyper, tokio 内部都用 Bytes）
// - 多个消费者共享同一份只读数据
// - 流水线处理（解析后传递 slice）
```

#### BytesMut 增长与 split

```rust
let mut buf = BytesMut::with_capacity(1024);
buf.put_u32(0xDEADBEEF);  // 写入
buf.put_slice(b"data");

// split: 把已写入部分"切"出来，零拷贝转 owned
let written: Bytes = buf.split();  // buf 继续可用，written 持有数据
// 内部用 Arc 共享，无 memcpy
```

#### Bytes vs Vec<u8> vs &[u8]

| 类型 | Clone 成本 | 所有权 | 何时用 |
|------|-----------|--------|--------|
| `&[u8]` | 不适用（借用） | 借用 | 函数参数，临时使用 |
| `Vec<u8>` | O(n) memcpy | owned | 需要修改、独立拥有 |
| `Bytes` | O(1) Arc clone | owned（共享） | 多消费者、跨 await、长期持有 |
| `BytesMut` | O(n) | owned | 需要修改的缓冲 |

### 4.3 Cow<'_, T>（写时复制）

```rust
use std::borrow::Cow;

// Cow 可以持有借用 &str 或 owned String，按需决定
fn normalize(input: &str) -> Cow<'_, str> {
    if input.chars().any(|c| c.is_uppercase()) {
        // 需要修改 → 分配 String（owned）
        Cow::Owned(input.to_lowercase())
    } else {
        // 无需修改 → 零拷贝借用
        Cow::Borrowed(input)
    }
}

let a = normalize("hello");  // Cow::Borrowed，零分配
let b = normalize("HeLLo");  // Cow::Owned，分配 String

// 适用：
// - 大多数情况下不需要改、少数情况下需要改
// - 解析器（保留原输入或转换）
// - 配置处理
```

### 4.4 避免无谓的 to_vec / to_owned

```rust
// ❌ 把 slice 转 Vec 只为传递
fn process(data: Vec<u8>) { /* ... */ }
process(buf.to_vec());  // memcpy！

// ✓ 接受 slice
fn process(data: &[u8]) { /* ... */ }
process(&buf);  // 零拷贝

// ❌ 函数内立即 clone
fn parse(data: &[u8]) -> Result {
    let owned = data.to_vec();  // 不必要的拷贝
    parse_inner(&owned)
}

// ✓ 直接借用
fn parse(data: &[u8]) -> Result {
    parse_inner(data)
}
```

## 5. L3：序列化零拷贝

### 5.1 zerocopy / bytemuck（结构体 ↔ 字节）

```rust
// 传统：序列化需要拷贝
let bytes = bincode::serialize(&struct)?;  // 分配 + 拷贝
let parsed: MyStruct = bincode::deserialize(&bytes)?;  // 拷贝

// ✓ zerocopy：直接把字节 reinterprete 为结构体，零拷贝
use zerocopy::{FromBytes, IntoBytes, Ref};

#[derive(FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
struct Header {
    magic: u32,
    version: u16,
    flags: u16,
    length: u32,
}

fn parse_header(buf: &[u8]) -> Option<&Header> {
    // 零拷贝！直接把 buf 的前 12 字节 reinterprete 为 &Header
    zerocopy::Ref::<&[u8], Header>::parse_prefix(buf).ok().map(|(h, _)| h.into_ref())
}

// 修改也是零拷贝
fn update_length(buf: &mut [u8], new_len: u32) {
    let header: &mut Header = zerocopy::Ref::parse_prefix(buf).ok()?.0.into_mut();
    header.length = new_len.to_be();  // 直接改 buf
}
```

#### zerocopy 的安全保证

```rust
// zerocopy 通过 derive 宏自动验证：
// - 类型是 #[repr(C)] 或 packed
// - 所有字段都是 POD（plain old data，无指针/生命周期）
// - 对齐正确

// ❌ 编译错误：String 不是 POD
#[derive(FromBytes)]  // 编译失败
struct Bad { name: String }

// ✓ 所有字段是 POD
#[derive(FromBytes, IntoBytes)]
#[repr(C)]
struct Good { x: u32, y: u32 }
```

### 5.2 bytemuck（类似 zerocopy，更成熟）

```rust
use bytemuck::{Pod, Zeroable, from_bytes, cast_slice};

#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
struct Vertex { x: f32, y: f32, z: f32 }

// 批量零拷贝转换
let bytes: &[u8] = read_file()?;
let vertices: &[Vertex] = cast_slice(bytes);  // 零拷贝 reinterprete
// 直接得到 &[Vertex]，无需 deserialize

// GPU 顶点缓冲、网络协议解析的利器
```

### 5.3 rkyv（零拷贝反序列化）

```rust
// serde 反序列化：分配新结构 + 拷贝字段
let data: MyStruct = serde_json::from_str(&json)?;  // 分配

// rkyv：序列化结果可以直接 reinterprete 为结构体引用，零拷贝
use rkyv::{Archive, Deserialize, Serialize};

#[derive(Archive, Deserialize, Serialize)]
struct Player {
    id: u32,
    name: String,
    scores: Vec<u32>,
}

// 序列化（一次）
let player = Player { id: 1, name: "Alice".into(), scores: vec![100, 200] };
let bytes = rkyv::to_bytes::<_, 256>(&player).unwrap();

// 反序列化（零拷贝！）
let archived = rkyv::from_bytes::<Player>(&bytes).unwrap();
// archived 是 &ArchivedPlayer，直接读 bytes 内存，不分配
println!("{}", archived.name.as_str());  // 零拷贝访问
```

### 5.4 序列化方案对比

| 方案 | 反序列化成本 | 适用场景 |
|------|-------------|---------|
| `serde_json` | 高（分配 + 解析） | 人类可读、配置 |
| `bincode` | 中（拷贝字段） | Rust 内部高效序列化 |
| `serde + serde_bytes` | 中 | 字节数组优化 |
| `zerocopy` / `bytemuck` | **零拷贝** | 固定布局、POD 结构 |
| `rkyv` | **零拷贝** | 持久化、网络传输、游戏存档 |
| `capnp` (Cap'n Proto) | **零拷贝** | 跨语言 RPC |

## 6. 字符串零拷贝

### 6.1 &str vs String

```rust
// ✓ 函数参数用 &str（接受 &String 和 &str）
fn greet(name: &str) { println!("Hello, {name}"); }

// 结构体字段：根据生命周期需求
struct Request<'a> {
    method: &'a str,  // 借用原始请求 buffer
    path: &'a str,
}
// 反序列化时零拷贝解析
```

### 6.2 serde 零拷贝字符串

```rust
use serde::{Deserialize, Serialize};

// 借用反序列化：从输入 &str 直接解析出 &str，不分配 String
#[derive(Deserialize)]
struct Config<'a> {
    #[serde(borrow)]
    name: &'a str,
    #[serde(borrow)]
    description: &'a str,
}

let json = r#"{"name":"app","description":"my app"}"#;
let config: Config = serde_json::from_str(json)?;
// config.name 直接指向 json 内部字节，零拷贝
```

### 6.3 CompactString / SmallString

```rust
use compact_str::CompactString;

// 短字符串（≤24 bytes）内联在栈，无堆分配
let s: CompactString = CompactString::from("hello");  // 栈上
let s2: CompactString = CompactString::from("a much longer string ...");  // 堆上

// 比 String 快 20-40%（多数字符串较短）
```

## 7. 实战模式：零拷贝网络协议解析

```rust
use bytes::{Bytes, Buf};
use zerocopy::FromBytes;

// 网络包格式：[4B magic][2B version][2B type][4B length][payload]

#[derive(FromBytes)]
#[repr(C)]
struct PacketHeader {
    magic: [u8; 4],
    version: u16,
    msg_type: u16,
    length: u32,
}

// 零拷贝解析整个数据流
fn parse_packet(buf: &Bytes) -> Option<Packet> {
    if buf.len() < 12 { return None; }

    // 1. 零拷贝解析 header（reinterprete 前 12 字节）
    let header = PacketHeader::ref_from_prefix(&buf[..12])?;
    if &header.magic != b"PKT\0" { return None; }

    // 2. 零拷贝提取 payload（slice，Arc 共享）
    let payload = buf.slice(12..);

    Some(Packet {
        version: header.version,
        msg_type: header.msg_type,
        payload,  // Bytes，O(1) clone
    })
}

struct Packet {
    version: u16,
    msg_type: u16,
    payload: Bytes,  // 引用计数共享
}
// 整个解析过程：0 次 memcpy，0 次堆分配（除了 Bytes 的 Arc）
```

## 8. 何时不要追求零拷贝

零拷贝不是银弹，以下场景**反而要拷贝**：

```rust
// 1. 数据会被频繁修改——零拷贝引用限制了可变性
//    如果后续要改，不如一开始就拷贝到 owned

// 2. 生命周期太短/复杂——借用让 API 难用
//    评估：零拷贝省的几次微秒 vs 开发成本的权衡

// 3. 数据很小（< 64 字节）——拷贝成本可忽略，借用开销更大
let flag: u32 = u32::from_le_bytes(buf[..4].try_into().unwrap());  // 拷贝，但只 4 字节

// 4. 跨线程/跨 await 持有——借用可能让 Future 非 Send
//    此时 owned + Arc<Bytes> 更合适

// 5. 安全风险——纯裸 reinterpret 在边界对齐错误时 UB
//    优先用 zerocopy/bytemuck 而非 unsafe 自写
```

## 9. 零拷贝检查清单

### 系统调用级
- [ ] 文件→网络用 `io::copy` / `sendfile`，不读入内存
- [ ] 大文件只读用 `mmap`
- [ ] 多缓冲区用 `write_vectored` / scatter-gather
- [ ] 高 IOPS 场景评估 `io_uring`

### 数据结构级
- [ ] 函数参数用 `&[u8]` 而非 `Vec<u8>`
- [ ] 多消费者用 `Bytes`（O(1) clone）
- [ ] 大多数只读、少数需改用 `Cow`
- [ ] 避免无谓的 `to_vec` / `to_owned`

### 序列化级
- [ ] POD 结构用 `zerocopy` / `bytemuck` 零拷贝转换
- [ ] 持久化数据用 `rkyv` 零拷贝反序列化
- [ ] serde 用 `#[serde(borrow)]` 借用反序列化
- [ ] 高性能 RPC 评估 `capnp`

### 字符串
- [ ] 函数参数用 `&str`
- [ ] 短字符串密集场景用 `CompactString`
- [ ] 解析器返回 `Cow<'_, str>`

### 安全
- [ ] 零拷贝 reinterpret 优先用安全库（zerocopy/bytemuck）
- [ ] 检查对齐和布局（`#[repr(C)]`）
- [ ] 不要为追求零拷贝引入 UB
