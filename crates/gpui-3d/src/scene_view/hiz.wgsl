// Conservative full-tile depth from proven solid boxes. Normal Z: 0 near, 1 far.
struct Frame { matrix:mat4x4<f32>, eye:vec3<f32> };
struct Box { lo:vec4<f32>, hi:vec4<f32> };
struct Boxes { count:vec4<u32>, boxes:array<Box> };
struct Level { value:vec4<u32> };
@group(0) @binding(0) var<uniform> frame:Frame;
@group(0) @binding(1) var<storage,read> boxes:Boxes;
@group(0) @binding(2) var<storage,read_write> depths:array<atomic<u32>>;
@group(0) @binding(3) var<uniform> level:Level;

fn offset(l:u32)->u32 { var sum=0u;for(var i=0u;i<l;i++){let size=128u>>i;sum+=size*size;}return sum; }
fn edge(a:vec2<f32>,b:vec2<f32>,p:vec2<f32>)->f32 {let d=b-a;let q=p-a;return d.x*q.y-d.y*q.x;}
fn inside(a:vec2<f32>,b:vec2<f32>,c:vec2<f32>,d:vec2<f32>,p:vec2<f32>)->bool {
    let e=vec4<f32>(edge(a,b,p),edge(b,c,p),edge(c,d,p),edge(d,a,p));
    return all(e>vec4<f32>(0.0001)) || all(e<vec4<f32>(-0.0001));
}
@compute @workgroup_size(64)
fn cs_hiz_clear(@builtin(global_invocation_id) id:vec3<u32>) {
    if(id.x<arrayLength(&depths)){atomicStore(&depths[id.x],bitcast<u32>(1.0));}
}
@compute @workgroup_size(64)
fn cs_hiz_raster(@builtin(workgroup_id) group:vec3<u32>,@builtin(local_invocation_index) lane:u32) {
    raster_box(group,lane,false);
}
@compute @workgroup_size(64)
fn cs_shadow_raster(@builtin(workgroup_id) group:vec3<u32>,@builtin(local_invocation_index) lane:u32) {
    raster_box(group,lane,true);
}
fn raster_box(group:vec3<u32>,lane:u32,shadow:bool) {
    let dimension=select(128u,512u,shadow);let extent=f32(dimension);
    let box_id=group.x/6u;if(box_id>=boxes.count.x){return;}
    let box=boxes.boxes[box_id];
    let face=group.x%6u;let axis=face/2u;let positive=(face&1u)!=0u;
    if((bitcast<u32>(box.hi.w)&(1u<<face))==0u){return;}
    let sign=select(-1.0,1.0,positive);let plane=select(box.lo[axis],box.hi[axis],positive);
    let forward=bitcast<vec3<f32>>(boxes.count.yzw);
    // Both tests are conservative for perspective and orthographic projections.
    if((frame.eye[axis]-plane)*sign<=0.0 || forward[axis]*sign>=0.0){return;}
    var p:array<vec3<f32>,8>;
    for(var i=0u;i<8u;i++){
        let world=select(box.lo.xyz,box.hi.xyz,vec3<bool>((i&1u)!=0u,(i&2u)!=0u,(i&4u)!=0u));
        let clip=frame.matrix*vec4<f32>(world,1.0);
        if(clip.w<=0.0001 || clip.z<0.0 || clip.z>clip.w){return;}
        let ndc=clip.xyz/clip.w;p[i]=vec3<f32>((ndc.xy*vec2<f32>(0.5,-0.5)+vec2<f32>(0.5))*extent,ndc.z);
    }
    let quads=array<vec4<u32>,6>(vec4<u32>(0u,4u,6u,2u),vec4<u32>(1u,3u,7u,5u),
        vec4<u32>(0u,1u,5u,4u),vec4<u32>(2u,6u,7u,3u),vec4<u32>(0u,2u,3u,1u),vec4<u32>(4u,5u,7u,6u));
    let t=quads[face];let a=p[t.x];let b=p[t.y];let c=p[t.z];let d=p[t.w];
    let lower=min(min(a.xy,b.xy),min(c.xy,d.xy));let upper=max(max(a.xy,b.xy),max(c.xy,d.xy));
    let lo=vec2<u32>(clamp(select(ceil(lower),floor(lower),shadow),vec2<f32>(0.0),vec2<f32>(extent)));
    let hi=vec2<u32>(clamp(select(floor(upper),ceil(upper),shadow),vec2<f32>(0.0),vec2<f32>(extent)));
    if(any(hi<=lo)){return;}
    let width=hi.x-lo.x;let count=width*(hi.y-lo.y);
    // Farthest vertex depth overestimates triangle depth over every fully covered tile.
    let depth=bitcast<u32>(max(max(a.z,b.z),max(c.z,d.z)));
    for(var i=lane;i<count;i+=64u){
        let tile=lo+vec2<u32>(i%width,i/width);let q=vec2<f32>(tile);
        if(shadow){
            let center=q+vec2<f32>(0.5);let area=edge(a.xy,b.xy,c.xy);
            if(abs(area)>0.000001 && inside(a.xy,b.xy,c.xy,d.xy,center)){
                let z=(edge(b.xy,c.xy,center)*a.z+edge(c.xy,a.xy,center)*b.z+edge(a.xy,b.xy,center)*c.z)/area;
                atomicMin(&depths[tile.y*dimension+tile.x],bitcast<u32>(clamp(z,0.0,1.0)));
            }
            continue;
        }
        if(inside(a.xy,b.xy,c.xy,d.xy,q) && inside(a.xy,b.xy,c.xy,d.xy,q+vec2<f32>(1.0,0.0))
            && inside(a.xy,b.xy,c.xy,d.xy,q+vec2<f32>(0.0,1.0)) && inside(a.xy,b.xy,c.xy,d.xy,q+vec2<f32>(1.0))){
            atomicMin(&depths[tile.y*128u+tile.x],depth);
        }
    }
}
@compute @workgroup_size(64)
fn cs_hiz_reduce(@builtin(global_invocation_id) id:vec3<u32>) {
    let l=level.value.x;let size=128u>>l;if(id.x>=size*size){return;}
    let p=vec2<u32>(id.x%size,id.x/size)*2u;let previous=size*2u;let base=offset(l-1u)+p.y*previous+p.x;
    let depth=max(max(atomicLoad(&depths[base]),atomicLoad(&depths[base+1u])),
        max(atomicLoad(&depths[base+previous]),atomicLoad(&depths[base+previous+1u])));
    atomicStore(&depths[offset(l)+id.x],depth);
}
