struct Vertex { x:f32, y:f32, z:f32, color:u32, u:f32, v:f32 };
struct Face { info:vec4<u32>, vertices:array<Vertex,4> };
// Each immutable template page is an array of 112-byte records. The first
// two records of each Section are headers; Compute sees a section-local view.
struct Templates {
    origin:vec4<f32>,
    origin_padding:array<vec4<u32>,6>,
    size:vec4<u32>,
    size_padding:array<vec4<u32>,6>,
    faces:array<Face>
};
struct GeneratedFace {cell:u32,model_face:u32,height:f32,padding:u32};
struct Params { size:vec4<u32>, origin:vec4<f32>, clip:vec4<f32>, extra:vec4<u32> };
@group(0) @binding(0) var<storage,read> cells:array<u32>;
@group(0) @binding(1) var<storage,read> templates:Templates;
@group(0) @binding(2) var<storage,read_write> output:array<GeneratedFace>;
@group(0) @binding(3) var<storage,read_write> args:array<atomic<u32>>;
@group(0) @binding(4) var<uniform> params:Params;
@group(0) @binding(5) var<storage,read_write> scratch:array<u32>;
// [0, jobs): instance prefix sums; [max(jobs, 1), ...): transient greedy
// masks. Six low bits mark exposed faces, six high bits mark faces certified
// for UV-preserving greedy merge. No host readback or new binding.
var<workgroup> prefix:array<u32,64>;
var<workgroup> output_base:u32;
fn opaque_cell(p:vec3<i32>,instance:bool)->bool {
    if(any(p<vec3<i32>(0)) || any(p>=vec3<i32>(params.size.xyz))){return false;}
    let index=(u32(p.z)*params.size.y+u32(p.y))*params.size.x+u32(p.x);
    if(!instance){return cells[index]!=0u;}
    let base=params.extra.w&0x7fffffffu;
    var kind=0u;
    if((params.extra.w&0x80000000u)!=0u){kind=(cells[base+index/4u]>>((index%4u)*8u))&255u;}
    else{kind=cells[base+index];}
    return (kind&bitcast<u32>(params.clip.w))!=0u;
}
fn face_ao(face:Face,cell:u32,instance:bool)->u32 {
    if(bitcast<u32>(params.clip.w)==0u){return 0u;}
    var low=vec3<f32>(1.0);var high=vec3<f32>(0.0);
    for(var i=0u;i<4u;i++){
        let v=face.vertices[i];let p=vec3<f32>(v.x,v.y,v.z);
        if(any((p!=vec3<f32>(0.0))&(p!=vec3<f32>(1.0)))){return 0u;}
        low=min(low,p);high=max(high,p);
    }
    var axis=3u;
    for(var a=0u;a<3u;a++){if(low[a]==high[a]){axis=a;}}
    if(axis==3u){return 0u;}
    let direction=select(-1,1,low[axis]==1.0);
    let normal=vec3<i32>(select(0,direction,axis==0u),select(0,direction,axis==1u),select(0,direction,axis==2u));
    let stride=vec3<i32>(1,i32(params.size.x),i32(params.size.x*params.size.y));
    if(bitcast<i32>(face.info.y)!=normal.x*stride.x+normal.y*stride.y+normal.z*stride.z){return 0u;}
    var tangents=vec2<u32>(0u,1u);
    if(axis==0u){tangents=vec2<u32>(1u,2u);}else if(axis==1u){tangents=vec2<u32>(0u,2u);}
    let p=vec3<i32>(i32(cell%params.size.x),i32(cell/params.size.x%params.size.y),i32(cell/params.size.x/params.size.y));
    var packed=0u;
    for(var i=0u;i<4u;i++){
        let v=face.vertices[i];let corner=vec3<f32>(v.x,v.y,v.z);
        let da=select(-1,1,corner[tangents.x]==1.0);
        let db=select(-1,1,corner[tangents.y]==1.0);
        let sa=normal+select(vec3<i32>(0),vec3<i32>(da),vec3<u32>(0u,1u,2u)==vec3<u32>(tangents.x));
        let sb=normal+select(vec3<i32>(0),vec3<i32>(db),vec3<u32>(0u,1u,2u)==vec3<u32>(tangents.y));
        let a=opaque_cell(p+sa,instance);let b=opaque_cell(p+sb,instance);
        var count=3u;
        if(!(a&&b)){count=u32(a)+u32(b)+u32(opaque_cell(p+sa+sb-normal,instance));}
        packed|=(255u-count*51u)<<(i*8u);
    }
    return packed;
}
fn instance_visible(face:Face,job:u32)->bool {
    let height=bitcast<f32>(cells[job+3u]);
    let y=params.origin.y+f32(cells[job+1u])+bitcast<f32>(face.info.w)*height;
    if(y<params.clip.x || y>=params.clip.y){return false;}
    let cell=(cells[job+2u]*params.size.y+cells[job+1u])*params.size.x+cells[job];
    if(face.info.y==0u){return true;}
    let neighbour=u32(i32(cell)+bitcast<i32>(face.info.y));
    var kind=0u;
    let class_base=params.extra.w&0x7fffffffu;
    if((params.extra.w&0x80000000u)!=0u){kind=(cells[class_base+neighbour/4u]>>((neighbour%4u)*8u))&255u;}
    else{kind=cells[class_base+neighbour];}
    return (kind&cells[job+7u])==0u;
}
// The high cell bit identifies a merged, canonical cube rectangle. The third
// descriptor word stores two u8 spans as an exactly represented integer f32; ordinary
// quads and arbitrary-model instance heights retain their original layout.
// Halo cells, static-face sentinel and instance descriptors are never tagged.
fn emit_face(face:Face,model_face:u32,position:vec3<f32>,cell:u32,extent:vec2<u32>){
    let y=position.y+bitcast<f32>(face.info.w);
    if(y<params.clip.x || y>=params.clip.y){return;}
    let vertex=atomicAdd(&args[face.info.x*4u],6u);
    let slot=(face.info.z&0x7fffffffu)+vertex/6u;
    let merged=extent.x>1u || extent.y>1u;
    let tagged=select(cell,cell|0x80000000u,merged);
    let packed=extent.x | (extent.y<<8u);
    // Values never exceed 0x1010, so f32 integer representation is exact
    // and cannot be flushed to zero as a subnormal on native backends.
    let scale=select(1.0,f32(packed),merged);
    // Every merged cell was certified to have four fully unoccluded AO corners.
    var ao=0u;
    if(merged && bitcast<u32>(params.clip.w)!=0u){ao=0xffffffffu;}
    else{ao=face_ao(face,cell,false);}
    output[slot]=GeneratedFace(tagged,model_face,scale,ao);
}
// Face repeatability is certified once while encoding immutable templates.
// face.info.z stores its bit 31, with the 31 low bits still the part base.
// Shader no longer checks four vertices for every potentially visible face.
// One writer per cell fills the mask only in mode 2 (large/complex sections).
fn build_greedy_mask(index:u32){
    let kind=cells[index];
    // No query ever reads a mask for empty/halo cells: exposed_cube checks the
    // source kind first. Sparse sections avoid thousands of zero writes.
    if(kind==0u || kind==0xffffffffu){return;}
    var mask=0u;
    let y=params.origin.y+f32(index/params.size.x%params.size.y);
    for(var direction=0u;direction<6u;direction++){
        let face=templates.faces[(kind-1u)*6u+direction];
        let neighbour=u32(i32(index)+bitcast<i32>(face.info.y));
        if(cells[neighbour]!=0u){continue;}
        mask|=1u<<direction;
        // Only canonical repeatable faces enter the greedy AO path.
        if((face.info.z&0x80000000u)==0u){continue;}
        let face_y=y+bitcast<f32>(face.info.w);
        if(face_y<params.clip.x || face_y>=params.clip.y){continue;}
        if(bitcast<u32>(params.clip.w)!=0u && face_ao(face,index,false)!=0xffffffffu){continue;}
        mask|=1u<<(direction+6u);
    }
    scratch[max(params.extra.x,1u)+index]=mask;
}
fn exposed_cube(cell:u32,kind:u32,direction:u32)->bool {
    if(cells[cell]!=kind){return false;}
    if(params.origin.w>=1.5){
        return (scratch[max(params.extra.x,1u)+cell]&(1u<<(direction+6u)))!=0u;
    }
    // Direct mode avoids a full-grid mask pass on cheap/dense sections. AO
    // requires the same exact per-corner rule as the cached mask builder.
    let face=templates.faces[(kind-1u)*6u+direction];
    let neighbour=u32(i32(cell)+bitcast<i32>(face.info.y));
    if(cells[neighbour]!=0u){return false;}
    let y=params.origin.y+f32(cell/params.size.x%params.size.y)+bitcast<f32>(face.info.w);
    if(y<params.clip.x || y>=params.clip.y){return false;}
    return bitcast<u32>(params.clip.w)==0u || face_ao(face,cell,false)==0xffffffffu;
}
fn face_row_width(cell:u32,kind:u32,direction:u32,stride:u32,coordinate:u32,dimension:u32)->u32 {
    let within=(coordinate-1u)%16u;
    var width=0u;
    for(var i=0u;i<16u;i++){
        if(within+i>=16u || coordinate+i+1u>=dimension){break;}
        if(!exposed_cube(cell+i*stride,kind,direction)){break;}
        width++;
    }
    return width;
}
// Check the exact required run and one possible continuation instead of
// rescanning up to 16 cells to compare the width of every adjacent row.
fn row_has_exact_width(cell:u32,kind:u32,direction:u32,stride:u32,
                       coordinate:u32,dimension:u32,width:u32)->bool {
    let available=min(16u-(coordinate-1u)%16u,dimension-coordinate-1u);
    if(width>available){return false;}
    for(var i=0u;i<width;i++){
        if(!exposed_cube(cell+i*stride,kind,direction)){return false;}
    }
    return width==available || !exposed_cube(cell+width*stride,kind,direction);
}
fn visible_rectangle(cell:u32,kind:u32,face:Face,direction:u32)->vec2<u32> {
    if(params.origin.w<0.5 || (face.info.z&0x80000000u)==0u){return vec2<u32>(1u);}
    if(!exposed_cube(cell,kind,direction)){return vec2<u32>(1u);}
    let delta=bitcast<i32>(face.info.y);
    let along_z=abs(delta)==1;
    let normal_y=abs(delta)==i32(params.size.x);
    let u_stride=select(1u,params.size.x*params.size.y,along_z);
    let v_stride=select(params.size.x,params.size.x*params.size.y,normal_y);
    let u=select(cell%params.size.x,cell/(params.size.x*params.size.y),along_z);
    let v=select(cell/params.size.x%params.size.y,cell/(params.size.x*params.size.y),normal_y);
    let u_size=select(params.size.x,params.size.z,along_z);
    let v_size=select(params.size.y,params.size.z,normal_y);
    let local_u=(u-1u)%16u;
    let local_v=(v-1u)%16u;
    if(local_u>0u && exposed_cube(cell-u_stride,kind,direction)){return vec2<u32>(0u);}
    let width=face_row_width(cell,kind,direction,u_stride,u,u_size);
    if(width==0u){return vec2<u32>(1u);}
    if(local_v>0u){
        let previous=cell-v_stride;
        let previous_starts=local_u==0u || !exposed_cube(previous-u_stride,kind,direction);
        if(previous_starts && row_has_exact_width(previous,kind,direction,u_stride,u,u_size,width)){
            return vec2<u32>(0u);
        }
    }
    var height=1u;
    for(var row=1u;row<16u;row++){
        if(local_v+row>=16u || v+row+1u>=v_size){break;}
        let next=cell+row*v_stride;
        if(local_u>0u && exposed_cube(next-u_stride,kind,direction)){break;}
        if(!row_has_exact_width(next,kind,direction,u_stride,u,u_size,width)){break;}
        height++;
    }
    return vec2<u32>(width,height);
}
@compute @workgroup_size(64)
fn cs_mesh(@builtin(workgroup_id) group:vec3<u32>, @builtin(num_workgroups) groups:vec3<u32>, @builtin(local_invocation_index) lane:u32) {
    mesh_stage(0u,group,groups,lane);
}
@compute @workgroup_size(64)
fn cs_mesh_greedy_mask(@builtin(workgroup_id) group:vec3<u32>, @builtin(num_workgroups) groups:vec3<u32>, @builtin(local_invocation_index) lane:u32){
    mesh_stage(6u,group,groups,lane);
}
@compute @workgroup_size(64)
fn cs_mesh_static_counts(@builtin(workgroup_id) group:vec3<u32>,@builtin(num_workgroups) groups:vec3<u32>,@builtin(local_invocation_index) lane:u32){mesh_stage(1u,group,groups,lane);}
@compute @workgroup_size(64)
fn cs_mesh_static_emit(@builtin(workgroup_id) group:vec3<u32>,@builtin(num_workgroups) groups:vec3<u32>,@builtin(local_invocation_index) lane:u32){mesh_stage(2u,group,groups,lane);}
@compute @workgroup_size(64)
fn cs_mesh_instance_counts(@builtin(workgroup_id) group:vec3<u32>,@builtin(num_workgroups) groups:vec3<u32>,@builtin(local_invocation_index) lane:u32){mesh_stage(3u,group,groups,lane);}
@compute @workgroup_size(64)
fn cs_mesh_instance_scan(@builtin(workgroup_id) group:vec3<u32>,@builtin(num_workgroups) groups:vec3<u32>,@builtin(local_invocation_index) lane:u32){mesh_stage(4u,group,groups,lane);}
@compute @workgroup_size(64)
fn cs_mesh_instance_emit(@builtin(workgroup_id) group:vec3<u32>,@builtin(num_workgroups) groups:vec3<u32>,@builtin(local_invocation_index) lane:u32){mesh_stage(5u,group,groups,lane);}
fn mesh_stage(mode:u32,group:vec3<u32>,groups:vec3<u32>,lane:u32){
    let index=(group.y*groups.x+group.x)*64u+lane;
    if(mode==4u){
        let part=group.y*groups.x+group.x;
        if(part>=arrayLength(&args)/4u){return;}
        let first=cells[params.extra.y+part*2u];
        let count=cells[params.extra.y+part*2u+1u];
        let stripe=(count+63u)/64u;
        let begin=min(count,lane*stripe);let end=min(count,begin+stripe);
        var sum=0u;for(var slot=begin;slot<end;slot++){sum+=scratch[first+slot];}
        prefix[lane]=sum;workgroupBarrier();
        for(var step=1u;step<64u;step*=2u){
            var add=0u;if(lane>=step){add=prefix[lane-step];}
            workgroupBarrier();prefix[lane]+=add;workgroupBarrier();
        }
        if(lane==0u){output_base=atomicAdd(&args[part*4u],prefix[63u]*6u)/6u;}
        workgroupBarrier();
        var offset=output_base;if(lane>0u){offset+=prefix[lane-1u];}
        for(var slot=begin;slot<end;slot++){let size=scratch[first+slot];scratch[first+slot]=offset;offset+=size;}
        return;
    }
    if(mode==3u || mode==5u){
        if(index>=params.extra.x){return;}
        let job=params.extra.z+index*8u;
        let first=cells[job+4u];let count=cells[job+5u];
        var emitted=0u;
        for(var local=0u;local<count;local++){
            let model_face=first+local;let face=templates.faces[model_face];
            if(!instance_visible(face,job)){continue;}
            if(mode==5u){
                let cell=(cells[job+2u]*params.size.y+cells[job+1u])*params.size.x+cells[job];
                let height=bitcast<f32>(cells[job+3u]);
                var ao=0u;if(height==1.0){ao=face_ao(face,cell,true);}
                output[(face.info.z&0x7fffffffu)+scratch[index]+emitted]=GeneratedFace(cell,model_face,height,ao);
            }
            emitted++;
        }
        if(mode==3u){scratch[index]=emitted;}
        return;
    }
    if(mode==1u){
        if(index>=arrayLength(&args)/4u){return;}
        let base=atomicLoad(&args[index*4u]);
        atomicStore(&args[index*4u+2u],base);
        atomicAdd(&args[index*4u],atomicLoad(&args[index*4u+3u])*6u);
        return;
    }
    if(mode==2u){
        if(index>=bitcast<u32>(params.clip.z)){return;}
        let model_face=params.size.w+index;
        let face=templates.faces[model_face];
        if(face.info.y==0xffffffffu){return;}
        let base=atomicLoad(&args[face.info.x*4u+2u])/6u;
        output[(face.info.z&0x7fffffffu)+base+face.info.y]=GeneratedFace(0xffffffffu,model_face,1.0,0u);
        return;
    }
    if(index>=params.size.x*params.size.y*params.size.z){return;}
    if(mode==6u){build_greedy_mask(index);return;}
    let kind=cells[index];
    if(kind==0u || kind==0xffffffffu){return;}
    let position=params.origin.xyz+vec3<f32>(f32(index%params.size.x),f32(index/params.size.x%params.size.y),f32(index/params.size.x/params.size.y));
    var exposed=0u;
    if(params.origin.w>=1.5){exposed=scratch[max(params.extra.x,1u)+index];}
    for(var direction=0u;direction<6u;direction++){
        if(params.origin.w>=1.5 && (exposed&(1u<<direction))==0u){continue;}
        let model_face=(kind-1u)*6u+direction;
        let face=templates.faces[model_face];
        if(params.origin.w<1.5){
            let neighbour=u32(i32(index)+bitcast<i32>(face.info.y));
            if(cells[neighbour]!=0u){continue;}
        }
        let extent=visible_rectangle(index,kind,face,direction);
        if(extent.x==0u){continue;}
        emit_face(face,model_face,position,index,extent);
    }
}
