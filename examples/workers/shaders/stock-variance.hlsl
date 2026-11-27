// fxc /T ps_5_0 /E main /Fo assets/stock-variance.cso shaders/stock-variance.hlsl
cbuffer Parameters : register(b0) {
    float mismatch;
};

float4 main(float2 uv : TEXCOORD0) : SV_Target {
    float4 track = float4(0.85, 0.89, 0.95, 1.0);
    float4 fill = float4(0.27, 0.18, 0.62, 1.0);
    float filled = 1.0 - step(saturate(mismatch), uv.x);
    return lerp(track, fill, filled);
}
