struct Vertex { x:f32, y:f32, z:f32, color:u32, u:f32, v:f32 };
struct Face { info:vec4<u32>, vertices:array<Vertex,4> };
struct Templates {origin:vec4<f32>,size:vec4<u32>,faces:array<Face>};
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
fn emit_face(face:Face,model_face:u32,position:vec3<f32>,cell:u32){
    let y=position.y+bitcast<f32>(face.info.w);
    if(y<params.clip.x || y>=params.clip.y){return;}
    let vertex=atomicAdd(&args[face.info.x*4u],6u);
    let slot=face.info.z+vertex/6u;
    output[slot]=GeneratedFace(cell,model_face,1.0,0u);
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
                output[face.info.z+scratch[index]+emitted]=GeneratedFace(cell,model_face,bitcast<f32>(cells[job+3u]),0u);
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
        emit_face(face,model_face,position,index);
    }
}
