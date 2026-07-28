# nethernet

Minecraft 基岩版 `NetherNet` 传输的 Rust 实现。

`NetherNet` 是基岩版较新版本使用的、基于 WebRTC 的点对点协议：局域网上先用
UDP 7551 端口做世界发现与信令交换，再在 WebRTC 的两条数据通道
（`ReliableDataChannel` / `UnreliableDataChannel`）上收发游戏报文。

线格式对齐
[Tianpao/GravityCone/lib/go-nethernet](https://github.com/Tianpao/GravityCone/tree/main/lib/go-nethernet)
与 vanilla 客户端。

## 模块

| 模块 | 职责 |
|---|---|
| `protocol` | 纯线格式编解码，无 IO：发现报文、`ServerData`、`Signal`、消息分片 |
| `signaling` | `Signaling` 抽象与局域网实现 `LanSignaling` |
| `session` | 数据通道之上的消息收发与生命周期 |
| `transport` | WebRTC 协商，对外提供 `NethernetListener` / `NethernetStream` |

## 线格式要点

发现报文：

```text
[HMAC-SHA256(明文) : 32] [AES-256-ECB(PKCS7(明文))]

明文 = u16le(总长, 含自身 2 字节)
       u16le(报文 ID) u64le(发送方网络 ID) [8 字节保留填充]
       body
```

密钥为 `SHA256(u64le(0xdeadbeef))`——协议自带的固定值，全网公开，只是混淆
而非加密。**读侧不校验长度前缀**：各实现对该字段的口径不一（含/不含自身
2 字节），go-nethernet 直接跳过，据此丢包只会造成互通失败。

`ServerData` v5 与 go-nethernet 逐字节一致（测试里有其测试向量），并兼容
旧版 v4 载荷。信令文本格式为 `TYPE ConnectionID Data`，`CONNECTERROR`
的完整错误码表见 `error::SignalErrorCode`。

## 绕开 webrtc-rs 的两处 64 KiB 硬限

`NetherNet` 的单分片上限是 262143 字节，而 webrtc-rs 0.17 有两处 64 KiB 限制：

1. **发送侧**：SCTP 单消息上限默认 65536，超过即 `ErrOutboundPacketTooLarge`。
   用 `SettingEngine::set_sctp_max_message_size_can_send` 放开到 262144。
2. **接收侧**：内置的 `RTCDataChannel` 读循环使用固定 65535 字节缓冲
   （`webrtc-0.17.2` `data_channel/mod.rs:33`），更大的入站消息会读失败并
   关闭通道，且**无法通过配置修改**。因此启用 `detach_data_channels`，由
   `session.rs` 按分片上限自管读循环。

不做这两件事时，任何 64 KiB 以上的游戏报文都无法送达——基岩版的区块批量
报文很容易超过该尺寸。`tests/loopback.rs` 里有对应的回归用例。

SDP 中还会补上 `a=max-message-size:262144`（webrtc-rs 不生成该属性，对端按
RFC 8841 会默认认为上限是 64 KiB，从而把自己的分片压小）。

## 零拷贝

全链路使用 `bytes::Bytes`：发现报文解密后一次成型，各字段是其切片视图；
单片消息直达上层不经过重组缓冲；出站分片用 `Bytes::slice` 切分，仅在拼接
分片头时拷贝一次。

## 并发与生命周期

- 每条数据通道一个读任务与**一条独立队列**。可靠通道承载有序游戏报文流
  （上层带加密计数器），丢一条就会让对端解密错位断线，因此积压时快速失败
  关闭会话；不可靠通道本就允许丢，积压即丢弃并计入 `SessionStats`。
  两条通道混用一个队列会让不可靠流量挤掉可靠消息，故必须分开。
- 出站按通道串行加锁，保证多分片消息不被交错。
- `recv` 的关闭信号是电平触发的 `CancellationToken`，可安全用于
  `tokio::select!`（边沿触发的 `Notify` 在 future 被取消重建时会漏通知）。
- **资源拆除**：webrtc-rs 全无 `Drop` 实现，不显式 `close()` 会永久泄漏
  ICE agent、绑定的 UDP 套接字与后台任务。因此：读任务只持 `Weak`（持强
  引用会让会话被自己的任务保活）、回调一律用 `Weak`（否则形成
  会话→对等连接→回调→会话 的环）、会话 `Drop` 时兜底拆除，且「逻辑关闭」
  与「资源拆除」用两个独立标志——对端先断开时逻辑关闭已置位，复用同一标志
  会让随后的 `close()` 短路。协商失败路径同样走完整拆除，否则伪造 offer
  可以耗尽监听器的并发协商槽。
- 发现层面向不可信输入：按源地址限速、发现表 TTL 与容量淘汰（满时淘汰最旧
  而非拒绝新对端，否则伪造网络 ID 灌满表即可拒绝所有合法连接）、信令长度
  上限、重组预留量封顶、并发协商数上限。
- 兼容 vanilla 在信令通道上周期发送的 `Ping` 报文（不计入丢包指标）。
