// fxc /T ps_5_0 /E main /Fo assets/scan-contrast.cso shaders/scan-contrast.hlsl
Texture2D image : register(t0);
SamplerState image_sampler : register(s0);

cbuffer Parameters : register(b0) {
    float contrast;
};

float4 main(float2 uv : TEXCOORD0) : SV_Target {
    float4 source = image.Sample(image_sampler, uv);
    float3 adjusted = saturate((source.rgb - 0.5) * contrast + 0.5);
    return float4(adjusted, 1.0);
}
