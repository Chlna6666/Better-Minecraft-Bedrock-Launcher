# nethernet

[中文](README.zh-CN.md)

Rust implementation of Minecraft Bedrock's NetherNet transport. NetherNet is the WebRTC-based
peer-to-peer protocol used by newer Bedrock versions: LAN discovery/signaling over UDP 7551,
then game traffic over the `ReliableDataChannel`/`UnreliableDataChannel` data channels.

The wire format is aligned with the vanilla client and the go-nethernet reference.

## Modules

| Module | Responsibility |
| --- | --- |
| `protocol` | Pure wire encoding/decoding: discovery packets, `ServerData`, `Signal`, fragmentation |
| `signaling` | `Signaling` abstraction and LAN implementation `LanSignaling` |
| `session` | Messaging and lifecycle on top of data channels |
| `transport` | WebRTC negotiation, `NethernetListener` / `NethernetStream` |

## Boundaries

- Protocol/transport only; no UI or application policy.
- Network setup and room policy stay in `src/core` and `easytier-bmcbl`.

## Build and test

```powershell
cargo check -p nethernet
cargo test -p nethernet
```

## Documentation

- [Architecture](../../docs/ARCHITECTURE.en.md)
- [Crate catalog](../../docs/CRATES.en.md)
