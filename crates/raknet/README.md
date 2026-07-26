# raknet（自研实现）

BMCBL 自研的 RakNet 协议栈，替代此前 vendored 的 bedrock-crustaceans/raknet。
连接模型参考 [go-raknet](https://github.com/Sandertv/go-raknet)，异步驱动层
保持与旧 raknet-tokio 相同的公开 API（`prelude::{RakClient, RakServer,
RakSession, RakReliability, RakPriority, ...}`），既有调用方无需改动。

## crate 结构

- `raknet`：协议核心（无 IO）。零拷贝编解码（`bytes::Bytes`）、
  可靠传输引擎（拆分/重组、有序/序列通道、可靠帧去重窗口、ACK/NACK、
  RTO 重传、AIMD 拥塞窗口、防护上限）。
- `raknet-tokio`：tokio 驱动。每套接字一个接收任务、每会话一个 tick
  任务；发送路径短锁直写套接字，无 actor 往返。

## 设计要点

- **零拷贝**：入站数据报 `recv_buf_from` 进 `BytesMut` 后 freeze，
  帧载荷是数据报的切片视图；出站载荷 `Bytes::slice` 拆分，仅在合帧进
  数据报时拷贝一次。`RakSession::{send_bytes, recv_bytes}` 提供全零拷贝
  API；`send/recv::<Box<[u8]>>` 保持旧接口兼容。
- **序号回绕安全**：u24 线上序号在内部展开为 u64，长连接不受 2^24
  回绕影响（数据报序号、可靠序号、有序/序列序号、ACK 范围均处理）。
- **重传模型**（go-raknet 风格）：重传使用新数据报序号；接收侧以
  可靠序号去重。RTO = SRTT + 4·RTTVAR + 20ms（RFC 6298 采样，Karn 规则），
  指数退避；NACK 触发快速重传。
- **go-raknet 互通**：客户端回显 OpenConnectionReply1 的 cookie
  （go-raknet v1.14+ 默认开启），客户端 GUID 强制符号位为 1
  （go-raknet 拒绝非负 GUID）。服务端 `security = true` 时同样签发
  并校验 cookie。地址编码遵循 RakNet 传统：IPv4 八位组按位取反、
  IPv6 family 小端。OpenConnectionRequest1 探测报文的 UDP 载荷
  = MTU − 28（固定口径），使 IP 包大小恰等于被探测的 MTU。
- **防护上限**：拆分重组（片数/并发组/字节）、乱序缓冲、ACK 记录数、
  离线报文限速（`packet_limit` / `total_packet_limit`，10ms 窗口）。
- **吞吐优化**：积压 ACK 提前冲刷（不等 10ms tick）、socket2 放大收发
  缓冲（Windows 默认 64KB 会在突发时丢包压制窗口）、`try_send_to`
  发送快路径。回环基准（900B 消息 ReliableOrdered，release）：
  78 MiB/s，RTT ~100µs（`cargo test -p raknet-tokio --release --test
  throughput -- --ignored --nocapture`）。

## 抗攻击设计要点

面向不可信对端的几处关键约束（均有回归测试）：

- 可靠帧去重按「已接收但前方有空洞」的**集合规模**设限，而非序号跨度。
  跨度上限会让超窗帧被静默丢弃却仍被 ACK，形成永不填补的空洞。
- 拆分重组按需插入分片（`HashMap`），不按对端声明的片数预分配；
  同 id 不同片数的帧直接丢弃而非重建条目。TTL 随新分片续期，
  慢速链路上的大消息不会被中途清除。
- ACK/NACK 处理与实际在途数据报求交，代价与在途数不成比例地
  跟随对端声明的范围宽度。NACK 触发的重传不计入链路死亡预算
  （NACK 未经认证）。
- 长期缓冲的小载荷会脱离入站数据报的父缓冲，
  否则字节上限会被放大数个量级。

## 与旧实现的行为修正

- ACK 解码 off-by-one（`start..end` 丢失末端序号）；
- keep-alive ConnectedPing 组包后从未发送；
- 可靠帧无去重窗口（重传会重复交付）；
- 拆分重组无任何上限（内存 DoS）；
- 定时基于 `SystemTime`（受系统时钟跳变影响，现用 `Instant`）。
