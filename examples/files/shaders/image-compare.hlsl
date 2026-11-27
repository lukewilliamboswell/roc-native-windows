// fxc /T ps_5_0 /E main /Fo assets/image-compare.cso shaders/image-compare.hlsl
Texture2D first_image : register(t0);
Texture2D second_image : register(t1);
SamplerState image_sampler : register(s0);

cbuffer Parameters : register(b0) {
    float divider;
};

float4 main(float2 uv : TEXCOORD0) : SV_Target {
    float4 first = first_image.Sample(image_sampler, uv);
    float4 second = second_image.Sample(image_sampler, uv);
    float4 blended = lerp(first, second, step(divider, uv.x));
    float guide = 1.0 - step(0.004, abs(uv.x - divider));
    return lerp(blended, float4(0.2, 0.72, 1.0, 1.0), guide);
}
