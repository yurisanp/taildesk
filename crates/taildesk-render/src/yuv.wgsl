// YUV (NV12 ou I420) -> RGBA, um triângulo que cobre a tela toda.

struct Cores {
    // Linhas da matriz YCbCr -> RGB e o deslocamento (faixa limitada ou cheia).
    r: vec4<f32>,
    g: vec4<f32>,
    b: vec4<f32>,
    deslocamento: vec4<f32>,
};

@group(0) @binding(0) var plano_y: texture_2d<f32>;
// NV12: Cb em .r e Cr em .g. I420: só Cb em .r.
@group(0) @binding(1) var plano_u: texture_2d<f32>;
// I420: Cr em .r. NV12 não usa.
@group(0) @binding(2) var plano_v: texture_2d<f32>;
@group(0) @binding(3) var amostra: sampler;
@group(0) @binding(4) var<uniform> cores: Cores;

struct Saida {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs(@builtin(vertex_index) i: u32) -> Saida {
    let x = f32((i << 1u) & 2u);
    let y = f32(i & 2u);
    var s: Saida;
    s.pos = vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
    s.uv = vec2<f32>(x, y);
    return s;
}

fn rgb(y: f32, cb: f32, cr: f32) -> vec4<f32> {
    let v = vec4<f32>(y, cb, cr, 1.0) - cores.deslocamento;
    let c = vec3<f32>(dot(cores.r, v), dot(cores.g, v), dot(cores.b, v));
    return vec4<f32>(clamp(c, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}

@fragment
fn fs_nv12(s: Saida) -> @location(0) vec4<f32> {
    let y = textureSample(plano_y, amostra, s.uv).r;
    let c = textureSample(plano_u, amostra, s.uv).rg;
    return rgb(y, c.x, c.y);
}

@fragment
fn fs_i420(s: Saida) -> @location(0) vec4<f32> {
    let y = textureSample(plano_y, amostra, s.uv).r;
    let cb = textureSample(plano_u, amostra, s.uv).r;
    let cr = textureSample(plano_v, amostra, s.uv).r;
    return rgb(y, cb, cr);
}
