// Pixel-space instanced quad shaders. Three pipelines share Globals:
//  - rect:  axis-aligned colored quads
//  - quad: arbitrary 4-corner colored quads (cursor blob)
//  - glyph: textured quads sampling the atlas alpha channel

struct Globals {
    resolution: vec2<f32>,
    offset: vec2<f32>,
};

@group(0) @binding(0) var<uniform> globals: Globals;

fn quad_corner(i: u32) -> vec2<f32> {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 1.0),
    );
    return corners[i];
}

fn to_clip(px: vec2<f32>) -> vec4<f32> {
    let p = px + globals.offset;
    let ndc = vec2<f32>(
        p.x / globals.resolution.x * 2.0 - 1.0,
        1.0 - p.y / globals.resolution.y * 2.0,
    );
    return vec4<f32>(ndc, 0.0, 1.0);
}

// ---------- rect pipeline ----------

struct RectIn {
    @location(0) pos: vec2<f32>,
    @location(1) size: vec2<f32>,
    @location(2) color: vec4<f32>,
};

struct RectOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn rect_vs(@builtin(vertex_index) vi: u32, in: RectIn) -> RectOut {
    let corner = quad_corner(vi);
    let px = in.pos + corner * in.size;
    var out: RectOut;
    out.clip = to_clip(px);
    out.color = in.color;
    return out;
}

@fragment
fn rect_fs(in: RectOut) -> @location(0) vec4<f32> {
    return vec4<f32>(in.color.rgb * in.color.a, in.color.a);
}

// ---------- quad pipeline (4 arbitrary corners) ----------

struct QuadIn {
    @location(0) c0: vec2<f32>,
    @location(1) c1: vec2<f32>,
    @location(2) c2: vec2<f32>,
    @location(3) c3: vec2<f32>,
    @location(4) color: vec4<f32>,
};

struct QuadOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
};

fn quad_tri_corner(vi: u32) -> u32 {
    var idx = array<u32, 6>(0u, 1u, 2u, 0u, 2u, 3u);
    return idx[vi];
}

@vertex
fn quad_vs(@builtin(vertex_index) vi: u32, in: QuadIn) -> QuadOut {
    let corners = array<vec2<f32>, 4>(in.c0, in.c1, in.c2, in.c3);
    let ci = quad_tri_corner(vi);
    var out: QuadOut;
    out.clip = to_clip(corners[ci]);
    out.color = in.color;
    return out;
}

@fragment
fn quad_fs(in: QuadOut) -> @location(0) vec4<f32> {
    return vec4<f32>(in.color.rgb * in.color.a, in.color.a);
}

// ---------- glyph pipeline ----------

struct GlyphIn {
    @location(0) pos: vec2<f32>,
    @location(1) size: vec2<f32>,
    @location(2) uv_min: vec2<f32>,
    @location(3) uv_max: vec2<f32>,
    @location(4) color: vec4<f32>,
};

struct GlyphOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
};

@group(1) @binding(0) var atlas_tex: texture_2d<f32>;
@group(1) @binding(1) var atlas_samp: sampler;

@vertex
fn glyph_vs(@builtin(vertex_index) vi: u32, in: GlyphIn) -> GlyphOut {
    let corner = quad_corner(vi);
    let px = in.pos + corner * in.size;
    var out: GlyphOut;
    out.clip = to_clip(px);
    out.uv = mix(in.uv_min, in.uv_max, corner);
    out.color = in.color;
    return out;
}

@fragment
fn glyph_fs(in: GlyphOut) -> @location(0) vec4<f32> {
    let a = textureSample(atlas_tex, atlas_samp, in.uv).r;
    let alpha = in.color.a * a;
    return vec4<f32>(in.color.rgb * alpha, alpha);
}
