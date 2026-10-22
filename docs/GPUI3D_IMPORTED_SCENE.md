# BMCBL 模型资产统一入口（tobj + gltf-rs + GPUI-3D）

## 模型导入责任边界

| 格式 | 文件解析 | 材质 / 场景资产 | 消费方 |
|---|---|---|---|
| OBJ / MTL | `bedrock-voxel` 内的 `tobj 4.0.3` 及现有安全三角化 | GPUI-3D `ImportedScene::from_parts` | 原始模型预览 / Minecraft 体素转换 |
| glTF / GLB | `gltf-rs 1.4.1`（关闭默认 `import` feature） | GPUI-3D `ImportedScene::from_gltf` | GPUI-3D 预览、场景数据遍历 |
| STL/FBX/3MF | 本阶段不启用 | 不伪造支持 | 后续评估 |

`three-d-asset` 不参与构建，避免其额外格式适配及重复 glTF 解析依赖。

### 关键数据所有权

- `ImportedScene` 持有不可变的 `Arc<Scene>`、`Arc<TextureAsset>`；`SceneView` 引用而非复制网格和纹理字节。
- `ImportedScene::for_each_triangle` 提供只读世界空间三角形访问，避免专门为 CPU 工具再建立顶点列表。回调需要在后台执行，不能在 GPUI Paint 热路径运行。
- OBJ 预览任务先用 `tobj` 读取受限 OBJ/MTL，再**只解码一次** MTL 的图片，放进 `Arc<[u8]>` 中。预览 GPU 资产和体素器共用同一份 CPU RGBA 字节；`TextureAsset::rgba8` 只克隆 Arc。
- 已存在的转换使用 `TaskManager`：UI 发布 task，文件读取/贴图解码使用 `run_io_blocking`，体素化和场景装配使用 `run_cpu`；进度与取消仍由现有任务协议提供，不允许 UI 管理后台 future。
- 大文件限制依然生效（OBJ 256 MiB、三角形 2M、单张贴图 64 MiB、解码贴图总量 256 MiB），同一路径重复引用的贴图只解码一次。

### glTF 解析迁移说明

GPUI-3D 不再使用手写 GLB chunk 和 glTF 文档解析器，而是通过 `gltf-rs` 读取并验证 glTF/GLB，保留原有 GPUI-3D accessor、稀疏数据、动画、材质、镜头及资源安全转换逻辑。此阶段仍需要通过 `serde_json::to_value(document.as_json())` 将 typed glTF 结构映射到原有转换实现，**并非已经完全迁移到 gltf-rs 零拷贝迭代器**。这是可测量的临时 CPU/内存开销，需要在实测后迁移转换器并移除旧 JSON-value 分支。

### 尚未删除的桥接代码

- 体素核心仍使用 `ObjModel` 存放高精度三角形；OBJ 预览使用最大 80,000 三角形的绘制采样网格。CPU 顶点缓存尚未完全零复制，因此**不应宣称全部模型几何只占一份内存**。
- GPUI-3D 的 `ImportedScene` 已具备材质及三角形遍历接口，但地图编辑器当前尚未把 glTF/GLB 方块转换 UI 切换至这一接口。不要把新统一资产入口等同于已有 glTF → `.mcstructure` 功能。
- 纹理 CPU RGBA 共享已消除 OBJ 预览任务内的重复解码；不同任务之间的全局纹理缓存仍需按路径、文件时间和内容版本设计，不可按路径盲目复用。

## 本地测试与体积对比

```powershell
cargo fmt --check
cargo test -p gpui-3d --lib
cargo test -p bedrock-voxel
cargo test --bin BMCBL --release
cargo build --release --locked
cargo tree -e features -i gltf
cargo tree -e features -i tobj
```

性能评估基线：单个 OBJ 与同内容 glTF/GLB 导入墙钟耗时、线程数、峰值 RSS、纹理解码次数、GPU 上传字节数、体素输出校验和、带动画模型正确性；比较前后 Release EXE 大小及 `cargo bloat`，不能仅按 crate 数宣称缩小。

如果四后端 Draw / Present 或自定义模型颜色出现变化，应优先回滚转换器行为而非削弱安全校验。
