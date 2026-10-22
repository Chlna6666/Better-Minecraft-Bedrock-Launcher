# 地图编辑器模型导入与导出

## 支持矩阵

| 位置 | 导入 | 导出 |
|---|---|---|
| 地图 → 模型转方块 | OBJ+MTL（贴图自动关联）、glTF、GLB | `.mcstructure`、OBJ+MTL+PNG、glTF、GLB |
| 地图 → 精确区块选区 | 现有地图区块读取 | OBJ+MTL+PNG、glTF、GLB |

`.mtl` 是 OBJ 的材质描述文件，不是独立网格格式；不能单独作为 3D 模型导入。OBJ 导出时同目录生成 `.mtl` 和 `*_material_N.png` 文件。

glTF (`.gltf`) 默认输出一个自包含 JSON 文件（`data:` URI 内嵌几何数据与贴图）；GLB (`.glb`) 输出一个自包含二进制文件。因而两种格式导出后可直接移动，不需要同时查找其它文件。glTF 的 Base64 文件体积一般高于 GLB。

## 解析与渲染

- OBJ：tobj（保留受限的 MTL、路径校验和三角化策略）。
- glTF/GLB：gltf-rs 1.4.1 解析与验证 glTF 文档，GPUI-3D 转换为统一 `ImportedScene`、`Mesh`、`Material`、`TextureAsset`。
- 原始 glTF/GLB 模型预览直接使用 GPUI-3D 原有节点、几何和材质，不重复生成第二个预览场景。
- 方块体素化借用同一份纹理 `Arc<[u8]>`，并使用已解析三角形。现有 OBJ 体素器保持统一的填充、调色板、透明阈值、取消规则。
- 相机在两侧对比预览中使用同一旋转和投影；按模型包围盒对原始模型进行归一化，以保证大尺寸 glTF 不被裁剪。

## 任务与性能

- 导入使用既有 TaskManager 后台 IO / CPU 工作池，界面不会解码纹理或三角化。
- `.mcstructure` 输出保留原有取消与进度。
- 导出源模型采用独立 TaskManager 工作任务。
- 精确选区导出由 TaskManager 接管，不再创建直接绑定 GPUI View 的持久导出 future。先用原有区块材质导出器构建 OBJ/MTL，再在私有临时目录中通过同一个 tobj 导入器转换为 glTF/GLB，完成后清理临时资源。
- OBJ+MTL 的多文件发布尚不属于完全原子事务；取消时优先清理 staging 文件，极少数 I/O/权限故障期间可能残留部分输出。
- glTF/GLB 输出是单文件临时写入再改名，带取消检查。

## 本地验收

```powershell
git pull origin master
cargo fmt --all --check
cargo test -p gpui-3d --lib
cargo test -p bedrock-voxel
cargo test --release --bin BMCBL
cargo build --release --locked
```

专项测试：OBJ+MTL+PNG 导入/导出；带纹理的 glTF/GLB 导入与导出后重导入；透明材质；源模型与体素结果同步相机；精确选区的 OBJ、glTF、GLB 导出；取消、失效外部资源、过大模型和高频刷新。测试峰值 RSS、纹理解码次数和 GPU 上传量。

## 已知限制

- 体素化表示和输出是静态网格，因此 glTF 骨骼与动画不会写入 Minecraft 方块；GPUI-3D 仍持有导入的动画数据。
- OBJ、glTF、GLB 的互相导出当前转换的是**静态三角形几何、UV、基础材质色、漫反射纹理与 Alpha**，不保留所有 PBR 扩展、皮肤骨骼、关键帧动画或完整纹理变换。
- 精确选区导出 glTF/GLB 为保证和既有 Minecraft 材质烘焙一致，暂用已有 OBJ/MTL 材质导出中间结果（在同一后台任务的私有临时目录内），而不是第二种格式解析库。
- glTF 导入后转换到方块时，顶点 COLOR_0 当前由 GPUI-3D 原始预览保留，但旧体素着色接口尚不支持逐顶点色插值，体素颜色以材质与贴图为主；不可将这视为完整的 glTF PBR 烘焙。
