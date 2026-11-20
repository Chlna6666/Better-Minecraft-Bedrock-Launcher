RWStructuredBuffer<uint> output_pixels : register(u0);
StructuredBuffer<uint> input_pixels : register(t0);

[numthreads(256, 1, 1)]
void main(uint3 id : SV_DispatchThreadID) {
    uint count;
    uint stride;
    output_pixels.GetDimensions(count, stride);
    if (id.x >= count) {
        return;
    }
    output_pixels[id.x] = input_pixels[id.x];
}
