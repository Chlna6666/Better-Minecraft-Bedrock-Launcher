# GPUI-3D 统一模型资产（第一阶段）

**目标**：格式导入一次、统一场景资产一份，GPUI-3D 渲染与 Minecraft 体素化读取相同的不可变几何和贴图，避免维护 OBJ/GLB/STL 的三套三角形算法。

## 稳定库及格式

- \`three-d-asset = 0.10.0\`（MIT），关闭默认特性，启用 \`obj\`、\`stl\` 和少量纹理解码器；不加入整套 three-d 图形引擎。
- \`.obj/.mtl\`、\`.stl\`：由 three-d-asset 的 CPU 资产解析器加载，再一次性构建 GPUI-3D \`Mesh\`/\`Material\`/\`TextureAsset\`。
- \`.gltf/.glb\`：**继续使用 GPUI-3D 内置 glTF 2.0 解析器**（无需 three-d-asset 再解一次），保留节点、材质、纹理、动画和相机，再封装为同一种 \`ImportedScene\`。
- \`3mf\`：three-d-asset 支持，但依赖可选 lib3mf，本阶段未启用，以免引入额外的原生部署负担。
- **FBX/PLY**：不属于已发布的 0.10.0 稳定版；不要错误地向用户宣称支持它们。上游 main/0.11 开发分支实现不能作为发布依据。

## 单一资产所有权

\`\`\`text
          文件（.obj / .stl / .glb / .gltf）
                         │
         ┌───────────────┴───────────────┐
         │                               │
 three-d-asset 0.10.0           GPUI-3D 原生 glTF
      OBJ / STL                      glTF / GLB
         │                               │
         └───────────────┬───────────────┘
                         ▼
                GPUI-3D::ImportedScene
            ┌────────────┴─────────────┐
         Arc<Scene>            共享 TextureAsset
    Node / Mesh / Material     CPU RGBA + GPU ID
            │
            ├── SceneView（持久缓存渲染，无重新解析）
            └── for_each_triangle（借用相同网格与像素；无新三角形 Vec）
                            │
                       CPU 体素化适配器
                 （仍待替换原 bedrock-voxel::ObjModel 入口）
\`\`\`

## API

\`\`\`rust
use gpui_3d::{Camera, ImportLimits, import_scene_path};

let scene = import_scene_path("/path/to/model.glb", &ImportLimits::default())?;
let view = scene.view(Camera::perspective(/* ... */))?;

// 下游 CPU 算法可以直接访问同一份 Mesh/TextureAsset，
// 并不需要解码一次 OBJ/MTL、分配另一批完整三角形。
scene.for_each_triangle(|triangle| {
    // triangle.positions / uvs / colors / material / albedo.pixels()
});
\`\`\`

调用方要在 TaskManager 的 IO/CPU Worker 执行导入，不要放在 GPUI UI 线程的 Render/Prepaint 中。

## 必须保留的现有安全边界

1. 目前地图编辑器原有 OBJ 方块转换仍走 \`bedrock-voxel::load_obj_model\`（tobj + 路径 confinement + concave face 校验）；**不可**在尚未迁移体素化适配器前删除。
2. 新统一导入接口在第一阶段只供受信任的本地模型使用：\`three-d-asset::io::load_and_deserialize\` 能递归读取 OBJ 外部资源，还没有完整复用 BMCBL 的 dependency confinement 和合并外部资源内存预算。**在这些安全限制闭合前，不向地图导入 UI 开放第三方不可信 OBJ 路径**。
3. 现有 \`bedrock-voxel\` 的 \`ObjModel\` 结构和 CPU 贴图采样还没有从统一 GPUI-3D 场景读取：**第一阶段仅统一 GPUI-3D 导入入口及借用三角形 API，并不宣称地图端已经完全零重复处理。**
4. 后续应为 \`bedrock-voxel\` 增加借用三角形/材质/像素采样的泛型接口，BMCBL 实现从 \`ImportedScene::for_each_triangle\` 到它的适配器；保留取消、8M 网格限制、色彩空间、UV wrapping、透明度和 \`mcstructure\` 导出逻辑。禁止为了兼容新格式额外解码贴图或把网格转换为临时 OBJ 文件。
5. glTF 默认场景可能包含皮肤/骨骼动画；方块化需要明确静态绑定姿势或选定时间帧，不能直接忽略骨骼变形并宣称无损。

## 本地检查

\`\`\`powershell
cargo check -p gpui-3d --features asset-import
cargo test -p gpui-3d --features asset-import imports_a_triangle_from_stl_via_three_d_asset
cargo test -p gpui-3d --features asset-import obj_source_is_imported_into_the_same_scene_type_as_stl
cargo check --bin BMCBL
\`\`\`

新增依赖将修改根 \`Cargo.lock\`；如果使用 \`--locked\` 构建，必须先使用本地 Rust 工具链重新生成 lockfile 并提交。不要依赖 CI 的自动修复代替本地测试。
