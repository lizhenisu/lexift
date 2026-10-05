cbuffer Parameters : register(b0) {
    float4 source_uv;
    float4 values;
};
Texture2D source_image : register(t0);
Texture2D mask_image : register(t1);
SamplerState linear_clamp : register(s0);

struct VertexOutput {
    float4 position : SV_POSITION;
    float2 uv : TEXCOORD0;
};

VertexOutput vertex_main(uint index : SV_VertexID) {
    VertexOutput output;
    float2 uv = float2((index << 1) & 2, index & 2);
    output.position = float4(uv.x * 2 - 1, 1 - uv.y * 2, 0, 1);
    output.uv = uv;
    return output;
}

float4 pixel_main(VertexOutput input) : SV_TARGET {
    return source_image.Sample(linear_clamp, source_uv.xy + input.uv * source_uv.zw) * values.x;
}

// One invocation per desktop-anchored cell, rather than one average per
// output pixel. Load keeps the average independent of texture filtering.
float4 grid_main(VertexOutput input) : SV_TARGET {
    int block = (int)values.z;
    int2 start = (int2(source_uv.xy) + int2(input.position.xy)) * block - int2(values.xy);
    uint width, height;
    source_image.GetDimensions(width, height);
    int2 first = max(start, int2(0, 0));
    int2 end = min(start + block, int2(width, height));
    float4 sum = 0;
    for (int y = first.y; y < end.y; ++y) {
        for (int x = first.x; x < end.x; ++x) {
            sum += source_image.Load(int3(x, y, 0));
        }
    }
    return sum / max(1, (end.x - first.x) * (end.y - first.y));
}

float4 expand_main(VertexOutput input) : SV_TARGET {
    int2 cell = int2(floor((source_uv.xy + input.position.xy) / values.z)) - int2(source_uv.zw);
    return source_image.Load(int3(cell, 0));
}

float4 mask_main(VertexOutput input) : SV_TARGET {
    float4 color = source_image.Sample(linear_clamp, source_uv.xy + input.uv * source_uv.zw);
    return color * mask_image.Sample(linear_clamp, input.uv).a;
}

float4 gaussian_main(VertexOutput input) : SV_TARGET {
    float4 sum = 0;
    float weight_sum = 0;
    int radius = (int)values.y;
    for (int i = -radius; i <= radius; ++i) {
        float weight = exp(-0.5 * i * i / (values.x * values.x));
        sum += source_image.SampleLevel(linear_clamp, input.uv + i * source_uv.xy * source_uv.zw, 0) * weight;
        weight_sum += weight;
    }
    return sum / weight_sum;
}

float4 down_main(VertexOutput input) : SV_TARGET {
    float2 d = source_uv.xy;
    float4 sum = source_image.SampleLevel(linear_clamp, input.uv, 0) * 4;
    sum += source_image.SampleLevel(linear_clamp, input.uv + float2(-d.x, -d.y), 0);
    sum += source_image.SampleLevel(linear_clamp, input.uv + float2(d.x, -d.y), 0);
    sum += source_image.SampleLevel(linear_clamp, input.uv + float2(-d.x, d.y), 0);
    sum += source_image.SampleLevel(linear_clamp, input.uv + d, 0);
    return sum / 8;
}

float4 up_main(VertexOutput input) : SV_TARGET {
    float2 d = source_uv.xy;
    float4 sum = source_image.SampleLevel(linear_clamp, input.uv + float2(-2 * d.x, 0), 0);
    sum += source_image.SampleLevel(linear_clamp, input.uv + float2(2 * d.x, 0), 0);
    sum += source_image.SampleLevel(linear_clamp, input.uv + float2(0, -2 * d.y), 0);
    sum += source_image.SampleLevel(linear_clamp, input.uv + float2(0, 2 * d.y), 0);
    sum += source_image.SampleLevel(linear_clamp, input.uv + float2(-d.x, -d.y), 0) * 2;
    sum += source_image.SampleLevel(linear_clamp, input.uv + float2(d.x, -d.y), 0) * 2;
    sum += source_image.SampleLevel(linear_clamp, input.uv + float2(-d.x, d.y), 0) * 2;
    sum += source_image.SampleLevel(linear_clamp, input.uv + d, 0) * 2;
    return sum / 12;
}

float4 mix_main(VertexOutput input) : SV_TARGET {
    return lerp(source_image.SampleLevel(linear_clamp, input.uv, 0),
        mask_image.SampleLevel(linear_clamp, input.uv, 0), values.x);
}

float4 lens_main(VertexOutput input) : SV_TARGET {
    float2 uv = source_uv.xy + input.uv * source_uv.zw;
    float4 color;
    if (values.x > 0) { color = source_image.SampleLevel(linear_clamp, uv, 0); }
    else {
        uint width, height;
        source_image.GetDimensions(width, height);
        int2 pixel = clamp(int2(floor(uv * float2(width, height))), int2(0, 0), int2(width - 1, height - 1));
        color = source_image.Load(int3(pixel, 0));
    }
    return color * mask_image.SampleLevel(linear_clamp, input.uv, 0).a;
}
