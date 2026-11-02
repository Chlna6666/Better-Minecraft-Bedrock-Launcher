@group(0) @binding(0) var<storage,read> args:array<u32>;
@vertex fn vs_mesh_check(@builtin(vertex_index) index:u32)->@builtin(position) vec4<f32>{
 let positions=array<vec2<f32>,3>(vec2<f32>(-1.0,-1.0),vec2<f32>(3.0,-1.0),vec2<f32>(-1.0,3.0));
 return vec4<f32>(positions[index],0.5,1.0);
}
@fragment fn fs_mesh_check()->@location(0) vec4<f32>{return vec4<f32>(f32(args[0]&255u),f32((args[0]>>8u)&255u),f32(args[1]),f32(args[3]))/255.0;}
@fragment fn fs_mesh_ao_check()->@location(0) vec4<f32>{
 let ao=args[3];
 return vec4<f32>(f32(ao&255u),f32((ao>>8u)&255u),f32((ao>>16u)&255u),f32((ao>>24u)&255u))/255.0;
}
@fragment fn fs_shadow_check()->@location(0) vec4<f32>{
 return vec4<f32>(bitcast<f32>(args[256u*512u+256u]),bitcast<f32>(args[0u]),bitcast<f32>(args[257u*512u+257u]),1.0);
}
