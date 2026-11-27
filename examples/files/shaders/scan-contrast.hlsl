// fxc /T ps_5_0 /E main /Fo assets/scan-contrast.cso shaders/scan-contrast.hlsl
Texture2D image : register(t0);
SamplerState image_sampler : register(s0);

cbuffer Parameters : register(b0) {
    float contrast;
};

float4 main(float2 uv : TEXCOORD0) : SV_Target {
    float4 source = image.Sample(image_sampler, uv);
    float3 straight = source.rgb / max(source.a, 1.0 / 255.0);
    float3 adjusted = saturate((straight - 0.5) * contrast + 0.5);
    return float4(adjusted * source.a, source.a);
}
