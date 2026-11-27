// Vertex shader for the platform-owned three-vertex fullscreen draw.
// fxc /T vs_5_0 /E main /Fo host/shaders/fullscreen.cso host/shaders/fullscreen.hlsl
struct VertexOutput {
    float4 position : SV_Position;
    float2 uv : TEXCOORD0;
};

VertexOutput main(uint vertex : SV_VertexID) {
    float2 uv = float2((vertex << 1) & 2, vertex & 2);
    VertexOutput output;
    output.position = float4(uv * float2(2.0, -2.0) + float2(-1.0, 1.0), 0.0, 1.0);
    output.uv = uv;
    return output;
}
