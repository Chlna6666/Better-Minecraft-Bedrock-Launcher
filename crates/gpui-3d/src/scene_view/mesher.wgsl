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
// Extend the generated ABI without adding a buffer: bit 31 of cell marks
// a horizontal merged run. The cell grid has at most i32::MAX entries, and
// the all-ones static-face sentinel remains reserved.
fn emit_face(face:Face,model_face:u32,position:vec3<f32>,cell:u32,span:u32){
    let y=position.y+bitcast<f32>(face.info.w);
    if(y<params.clip.x || y>=params.clip.y){return;}
    let vertex=atomicAdd(&args[face.info.x*4u],6u);
    let slot=face.info.z+vertex/6u;
    let tagged=select(cell,cell|0x80000000u,span>1u);
    output[slot]=GeneratedFace(tagged,model_face,f32(span),face_ao(face,cell,false));
}
// A safe horizontal run contains only already-exposed faces of the SAME
// six-face material/model. No hidden face is materialized before merging.
// A 16-cell tile cap bounds each GPU invocation's traversal cost.
fn visible_run(cell:u32,kind:u32,face:Face)->u32 {
    if(params.origin.w<0.5){return 1u;}
    for(var i=0u;i<4u;i++){
        let v=face.vertices[i];
        let p=vec3<f32>(v.x,v.y,v.z);
        if(any((p!=vec3<f32>(0.0))&(p!=vec3<f32>(1.0)))){return 1u;}
    }
    let delta=bitcast<i32>(face.info.y);
    // For +/-X faces the horizontal tangent is Z. For Y/Z faces use X.
    let along_z=delta==1 || delta==-1;
    let stride=select(1u,params.size.x*params.size.y,along_z);
    let coordinate=select(cell%params.size.x,cell/(params.size.x*params.size.y),along_z);
    let dimension=select(params.size.x,params.size.z,along_z);
    let within_tile=(coordinate-1u)%16u;
    // Only the first exposed cell of a row run may output its merged quad.
    if(within_tile>0u && coordinate>1u){
        let previous=cell-stride;
        if(cells[previous]==kind && cells[u32(i32(previous)+delta)]==0u){return 0u;}
    }
    var span=1u;
    for(var step=1u;step<16u;step++){
        if(within_tile+step>=16u || coordinate+step+1u>=dimension){break;}
        let next=cell+step*stride;
        if(cells[next]!=kind || cells[u32(i32(next)+delta)]!=0u){break;}
        span++;
    }
    return span;
}
@compute @workgroup_size(64)
fn cs_mesh(@builtin(workgroup_id) group:vec3<u32>, @builtin(num_workgroups) groups:vec3<u32>, @builtin(local_invocation_index) lane:u32) {
    mesh_stage(0u,group,groups,lane);
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
                output[face.info.z+scratch[index]+emitted]=GeneratedFace(cell,model_face,height,ao);
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
        output[face.info.z+base+face.info.y]=GeneratedFace(0xffffffffu,model_face,1.0,0u);
        return;
    }
    if(index>=params.size.x*params.size.y*params.size.z){return;}
    let kind=cells[index];
    if(kind==0u || kind==0xffffffffu){return;}
    let position=params.origin.xyz+vec3<f32>(f32(index%params.size.x),f32(index/params.size.x%params.size.y),f32(index/params.size.x/params.size.y));
    for(var direction=0u;direction<6u;direction++){
        let model_face=(kind-1u)*6u+direction;
        let face=templates.faces[model_face];
        let neighbour=u32(i32(index)+bitcast<i32>(face.info.y));
        if(cells[neighbour]!=0u){continue;}
        let span=visible_run(index,kind,face);
        if(span==0u){continue;}
        emit_face(face,model_face,position,index,span);
    }
}
