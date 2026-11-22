//! The 3D canvas's GL pass, inside egui's paint callback on the glow
//! backend (`egui_glow::CallbackFn`): the scene into an offscreen target,
//! a bright pass of what is lit, a separable blur of it at half size, and
//! the composite onto egui's framebuffer.
//!
//! This is the one module of the canvas that holds `unsafe`: glow's calls
//! are `unsafe fn`s (GL's state is global and a bad handle is the caller's
//! to avoid), and the card asks for our own GL pass. Every call is in a
//! block with its safety comment, and the crate denies `unsafe_code`
//! everywhere else (main.rs). The contract for all of them is the same:
//!
//! - the context is the one `egui_glow::Painter` hands the callback, on
//!   the thread that is painting, with its context current;
//! - every handle used was made by this module from that context and is
//!   deleted only by [`destroy`], which the app calls on exit and when the
//!   view is switched off;
//! - what egui expects afterwards is put back: the framebuffer, the
//!   scissor, depth and blend state, the viewport, the buffer and vertex
//!   array bindings. egui's own `prepare_painting` re-applies the rest
//!   (program, blend function, active texture) after every callback.
//!
//! The shaders take their version line from `egui_glow`'s `ShaderVersion`,
//! so they compile as GLSL 1.40 on a desktop and as GLSL ES 3.00 in the
//! browser (WebGL2). A context older than that (GLSL 1.20, ES 1.00, which
//! is `WebGL1`) has no `in`/`out` and no `gl_VertexID`: the pass reports it
//! and the view stays 2D.
#![allow(unsafe_code)]
// GL takes sizes as i32 and enums as u32; ours are small and known.
#![allow(clippy::cast_possible_wrap)]

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use eframe::egui::PaintCallbackInfo;
use eframe::egui_glow::{Painter, ShaderVersion};
use eframe::glow::{self, HasContext as _};

use crate::batch3d::{Batch, LINE_STRIDE, POOL_STRIDE, SOLID_STRIDE};
use crate::cam3d::{M4, V3};

/// What one frame draws.
pub struct Frame {
    pub vp: M4,
    pub eye: V3,
    pub batch: Batch,
    /// The page's colour, as the theme has it (sRGB).
    pub bg: [f32; 3],
    /// Fog: the page's colour, linear, and the reciprocal of the distance
    /// where it takes hold.
    pub fog: [f32; 4],
    /// The clock the pulses run on, seconds.
    pub time: f32,
    /// How much of the blurred light is added, and the exposure.
    pub bloom: f32,
    pub exposure: f32,
    /// Physical pixels to a point: the lines' widths are in points.
    pub ppp: f32,
}

/// The state the app and its paint callbacks share: the GL objects (made
/// on the first frame that paints), why there are none, and a request to
/// free them.
#[derive(Default)]
pub struct Shared {
    res: Mutex<Option<Res>>,
    error: Mutex<Option<String>>,
    release: AtomicBool,
}

impl Shared {
    /// Why the pass cannot draw (a shader that did not compile, a context
    /// too old), once known.
    #[must_use]
    pub fn error(&self) -> Option<String> {
        self.error.lock().ok().and_then(|e| e.clone())
    }

    /// Asks the next callback to free the GL objects.
    pub fn request_release(&self) {
        self.release.store(true, Ordering::SeqCst);
    }

    /// Whether a release is asked and not yet done.
    #[must_use]
    pub fn release_pending(&self) -> bool {
        self.release.load(Ordering::SeqCst)
    }

    /// Frees the GL objects with `gl` (the app's exit, which has the
    /// context but no painter).
    pub fn destroy(&self, gl: &glow::Context) {
        if let Ok(mut r) = self.res.lock()
            && let Some(res) = r.take()
        {
            res.delete(gl);
        }
        self.release.store(false, Ordering::SeqCst);
    }

    /// The paint callback's body.
    pub fn paint(&self, info: &PaintCallbackInfo, painter: &Painter, frame: &Frame) {
        let gl: &glow::Context = painter.gl();
        let Ok(mut res) = self.res.lock() else { return };
        if self.release.swap(false, Ordering::SeqCst) {
            if let Some(r) = res.take() {
                r.delete(gl);
            }
            return;
        }
        if res.is_none() {
            match Res::new(gl) {
                Ok(r) => *res = Some(r),
                Err(e) => {
                    if let Ok(mut slot) = self.error.lock() {
                        *slot = Some(e);
                    }
                    return;
                }
            }
        }
        let vp = info.viewport_in_pixels();
        let size = (vp.width_px, vp.height_px);
        if size.0 < 1 || size.1 < 1 {
            return;
        }
        let Some(r) = res.as_mut() else { return };
        if let Err(e) = r.draw(
            gl,
            frame,
            size,
            (vp.left_px, vp.from_bottom_px),
            painter.intermediate_fbo(),
        ) {
            if let Some(r) = res.take() {
                r.delete(gl);
            }
            if let Ok(mut slot) = self.error.lock() {
                *slot = Some(e);
            }
        }
    }
}

const SOLID_VS: &str = "
uniform mat4 u_vp;
in vec3 a_pos;
in vec3 a_nrm;
in vec4 a_col;
in float a_emit;
in vec3 a_ctr;
in vec3 a_half;
in float a_mat;
out vec3 v_pos;
out vec3 v_nrm;
out vec4 v_col;
out float v_emit;
out vec3 v_ctr;
out vec3 v_half;
out float v_mat;
void main() {
    v_pos = a_pos;
    v_nrm = a_nrm;
    v_col = a_col;
    v_emit = a_emit;
    v_ctr = a_ctr;
    v_half = a_half;
    v_mat = a_mat;
    gl_Position = u_vp * vec4(a_pos, 1.0);
}
";

const SOLID_FS: &str = "
uniform vec3 u_eye;
uniform vec4 u_fog;
in vec3 v_pos;
in vec3 v_nrm;
in vec4 v_col;
in float v_emit;
in vec3 v_ctr;
in vec3 v_half;
in float v_mat;
out vec4 o_col;
void main() {
    vec3 n = normalize(v_nrm);
    vec3 view = normalize(u_eye - v_pos);
    float dist = length(u_eye - v_pos);
    vec3 key = normalize(vec3(-0.35, 0.85, 0.4));
    vec3 fill = normalize(vec3(0.6, 0.25, -0.5));
    float hemi = 0.5 + 0.5 * n.y;
    float diffuse = 0.34 * hemi + 0.72 * max(dot(n, key), 0.0) + 0.18 * max(dot(n, fill), 0.0);
    vec3 base = v_col.rgb;
    // Distance to the nearest edge of the face this fragment is on: a
    // bevel that catches the light.
    vec3 gap = v_half - abs(v_pos - v_ctr);
    vec3 an = abs(n);
    float edge = min(an.x > 0.5 ? 1e3 : gap.x, min(an.y > 0.5 ? 1e3 : gap.y, an.z > 0.5 ? 1e3 : gap.z));
    float bevel = 1.0 - smoothstep(0.0, 0.07, edge);
    float rim = pow(1.0 - max(dot(n, view), 0.0), 3.0);
    vec3 shaded = base * diffuse;
    float gloss = 0.0;
    if (v_mat > 0.5 && v_mat < 1.5) {
        // The board: a faint grid, and a sheen that grows toward the
        // horizon, like a glossy floor.
        vec2 g = abs(fract(v_pos.xz) - 0.5);
        float line = smoothstep(0.46, 0.5, max(g.x, g.y));
        float fade = exp(-dist * 0.03);
        shaded += base * line * 1.6 * fade * step(0.5, n.y);
        gloss = pow(1.0 - max(dot(n, view), 0.0), 4.0) * step(0.5, n.y);
        shaded += u_fog.rgb * gloss * 3.0 + vec3(0.02, 0.03, 0.04) * gloss;
    } else {
        shaded += base * bevel * 0.5;
        shaded += base * rim * 0.25;
        if (v_mat > 2.5) {
            shaded += base * bevel * 0.6;
        }
    }
    float f = 1.0 - exp(-pow(dist * u_fog.w, 2.0));
    vec3 rgb = mix(shaded, u_fog.rgb, f * 0.85) + v_col.rgb * v_emit * (1.0 - 0.6 * f) * (0.55 + 0.45 * bevel + 0.2);
    o_col = vec4(rgb * v_col.a, v_col.a);
}
";

const LINE_VS: &str = "
uniform mat4 u_vp;
uniform vec2 u_res;
uniform float u_ppp;
in vec3 a_a;
in vec3 a_b;
in vec2 a_corner;
in vec4 a_col;
in float a_width;
in float a_emit;
in float a_phase;
in float a_flow;
out vec4 v_col;
out float v_emit;
out float v_across;
out float v_along;
out float v_flow;
out float v_w;
out float v_depth;
void main() {
    vec4 ca = u_vp * vec4(a_a, 1.0);
    vec4 cb = u_vp * vec4(a_b, 1.0);
    float near = 0.05;
    v_col = a_col;
    v_emit = a_emit;
    v_across = a_corner.y;
    v_along = a_phase + a_corner.x * length(a_b - a_a);
    v_flow = a_flow;
    v_w = max(a_width * u_ppp, 2.0);
    if (ca.w < near && cb.w < near) {
        v_depth = 1.0;
        gl_Position = vec4(2.0, 2.0, 2.0, 1.0);
        return;
    }
    if (ca.w < near) {
        ca = mix(ca, cb, (near - ca.w) / (cb.w - ca.w));
    } else if (cb.w < near) {
        cb = mix(cb, ca, (near - cb.w) / (ca.w - cb.w));
    }
    vec2 pa = ca.xy / ca.w * u_res * 0.5;
    vec2 pb = cb.xy / cb.w * u_res * 0.5;
    vec2 d = pb - pa;
    float len = length(d);
    vec2 dir = len > 0.0001 ? d / len : vec2(1.0, 0.0);
    vec2 nrm = vec2(-dir.y, dir.x);
    vec4 c = mix(ca, cb, a_corner.x);
    float half_w = v_w * 0.5 + 1.0;
    vec2 off = (nrm * a_corner.y + dir * (a_corner.x * 2.0 - 1.0)) * half_w * 2.0 / u_res;
    v_across = a_corner.y * half_w / (v_w * 0.5);
    v_depth = c.w;
    gl_Position = vec4(c.xy + off * c.w, c.z - 0.0006 * c.w, c.w);
}
";

const LINE_FS: &str = "
uniform float u_time;
uniform vec4 u_fog;
in vec4 v_col;
in float v_emit;
in float v_across;
in float v_along;
in float v_flow;
in float v_w;
in float v_depth;
out vec4 o_col;
void main() {
    float d = abs(v_across);
    float cov = 1.0 - smoothstep(1.0 - 2.0 / v_w, 1.0 + 1.0 / v_w, d);
    float core = 1.0 - smoothstep(0.0, 0.75, d);
    // Pulses run along the wire in the direction of the data.
    float ph = fract(v_along / 2.4 - u_time);
    float head = pow(1.0 - ph, 5.0) * smoothstep(0.0, 0.04, ph);
    float emit = v_emit * (1.0 + v_flow * 2.6 * head);
    vec3 rgb = v_col.rgb * (0.55 + 0.45 * core) + v_col.rgb * emit * (0.6 + 0.8 * core);
    float f = 1.0 - exp(-pow(v_depth * u_fog.w, 2.0));
    rgb = mix(rgb, u_fog.rgb, f * 0.6);
    float a = cov * v_col.a;
    o_col = vec4(rgb * a, a);
}
";

const POOL_VS: &str = "
uniform mat4 u_vp;
in vec3 a_pos;
in vec2 a_uv;
in vec3 a_col;
out vec2 v_uv;
out vec3 v_col;
void main() {
    v_uv = a_uv;
    v_col = a_col;
    gl_Position = u_vp * vec4(a_pos, 1.0);
}
";

const POOL_FS: &str = "
in vec2 v_uv;
in vec3 v_col;
out vec4 o_col;
void main() {
    float f = clamp((1.0 - abs(v_uv.x)) * (1.0 - abs(v_uv.y)), 0.0, 1.0);
    f = f * f * (3.0 - 2.0 * f);
    o_col = vec4(v_col * f * f, 0.0);
}
";

const QUAD_VS: &str = "
out vec2 v_uv;
void main() {
    vec2 p = vec2(float((gl_VertexID << 1) & 2), float(gl_VertexID & 2));
    v_uv = p;
    gl_Position = vec4(p * 2.0 - 1.0, 0.0, 1.0);
}
";

const BRIGHT_FS: &str = "
uniform sampler2D u_tex;
uniform vec2 u_texel;
uniform float u_thr;
in vec2 v_uv;
out vec4 o_col;
vec3 prefilter(vec3 c) {
    float m = max(c.r, max(c.g, c.b));
    float w = max(m - u_thr, 0.0) / max(m, 0.0001);
    return c * w;
}
void main() {
    vec3 c = prefilter(texture(u_tex, v_uv + u_texel * vec2(-0.5, -0.5)).rgb)
        + prefilter(texture(u_tex, v_uv + u_texel * vec2(0.5, -0.5)).rgb)
        + prefilter(texture(u_tex, v_uv + u_texel * vec2(-0.5, 0.5)).rgb)
        + prefilter(texture(u_tex, v_uv + u_texel * vec2(0.5, 0.5)).rgb);
    o_col = vec4(c * 0.25, 1.0);
}
";

const DOWN_FS: &str = "
uniform sampler2D u_tex;
uniform vec2 u_texel;
in vec2 v_uv;
out vec4 o_col;
void main() {
    vec3 c = texture(u_tex, v_uv + u_texel * vec2(-1.0, -1.0)).rgb
        + texture(u_tex, v_uv + u_texel * vec2(1.0, -1.0)).rgb
        + texture(u_tex, v_uv + u_texel * vec2(-1.0, 1.0)).rgb
        + texture(u_tex, v_uv + u_texel * vec2(1.0, 1.0)).rgb;
    o_col = vec4(c * 0.25, 1.0);
}
";

const UP_FS: &str = "
uniform sampler2D u_base;
uniform sampler2D u_lo;
uniform vec2 u_texel;
uniform float u_k;
in vec2 v_uv;
out vec4 o_col;
void main() {
    vec3 t = texture(u_lo, v_uv).rgb * 4.0;
    t += texture(u_lo, v_uv + u_texel * vec2(-1.0, 0.0)).rgb * 2.0;
    t += texture(u_lo, v_uv + u_texel * vec2(1.0, 0.0)).rgb * 2.0;
    t += texture(u_lo, v_uv + u_texel * vec2(0.0, -1.0)).rgb * 2.0;
    t += texture(u_lo, v_uv + u_texel * vec2(0.0, 1.0)).rgb * 2.0;
    t += texture(u_lo, v_uv + u_texel * vec2(-1.0, -1.0)).rgb;
    t += texture(u_lo, v_uv + u_texel * vec2(1.0, -1.0)).rgb;
    t += texture(u_lo, v_uv + u_texel * vec2(-1.0, 1.0)).rgb;
    t += texture(u_lo, v_uv + u_texel * vec2(1.0, 1.0)).rgb;
    o_col = vec4(texture(u_base, v_uv).rgb + t * (u_k / 16.0), 1.0);
}
";

const COMPOSITE_FS: &str = "
uniform sampler2D u_scene;
uniform sampler2D u_bloom;
uniform float u_bloom_k;
uniform float u_exposure;
uniform vec3 u_bg;
in vec2 v_uv;
out vec4 o_col;
vec3 aces(vec3 x) {
    return clamp((x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14), 0.0, 1.0);
}
float hash(vec2 p) {
    return fract(sin(dot(p, vec2(12.9898, 78.233))) * 43758.5453);
}
void main() {
    vec4 s = texture(u_scene, v_uv);
    vec3 bl = texture(u_bloom, v_uv).rgb * u_bloom_k;
    vec3 hdr = (s.rgb + bl) * u_exposure;
    vec3 tm = aces(hdr);
    vec3 enc = pow(tm, vec3(1.0 / 2.2));
    // Where nothing is drawn the page shows, less where light spills.
    float spill = clamp(dot(tm, vec3(0.333)) * 1.5, 0.0, 1.0);
    vec3 rgb = enc + u_bg * (1.0 - s.a) * (1.0 - spill);
    rgb += (hash(gl_FragCoord.xy) - 0.5) / 255.0;
    o_col = vec4(rgb, 1.0);
}
";

/// A linked program.
struct Prog(glow::Program);

/// The pixel format of the scene and the bloom: half floats where the
/// context can render to them, else bytes (checked at startup).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Format {
    Half,
    Byte,
}

/// An offscreen target: colour texture, optional depth.
struct Target {
    fbo: glow::Framebuffer,
    tex: glow::Texture,
    depth: Option<glow::Renderbuffer>,
    size: (i32, i32),
}

/// Levels of the bloom's chain: each half the size of the one before,
/// the first half the size of the view.
const LEVELS: usize = 5;

struct Res {
    fmt: Format,
    vao: glow::VertexArray,
    vbo: glow::Buffer,
    solid: Prog,
    line: Prog,
    pool: Prog,
    bright: Prog,
    down: Prog,
    up: Prog,
    composite: Prog,
    scene: Option<Target>,
    /// The way down: bright pass, then each level a blur of the last.
    downs: Vec<Target>,
    /// The way up: each level plus the one below it, widened.
    ups: Vec<Target>,
}

fn header(gl: &glow::Context) -> Result<String, String> {
    // The one GL query (a string), on the painting context.
    let v = ShaderVersion::get(gl);
    if !v.is_new_shader_interface() {
        return Err("3D needs WebGL2 or OpenGL 3: this context is older".into());
    }
    let mut h = v.version_declaration().to_string();
    if v.is_embedded() {
        h.push_str("precision highp float;\nprecision highp sampler2D;\n");
    }
    Ok(h)
}

fn compile(gl: &glow::Context, header: &str, vs: &str, fs: &str, attribs: &[&str]) -> Result<Prog, String> {
    // SAFETY: shaders and the program are made, attached, linked and the
    // shaders deleted here, on the painting context; the handles do not
    // outlive the call except the program, which `Res::delete` frees.
    unsafe {
        let program = gl.create_program()?;
        let mut shaders = Vec::new();
        for (kind, src) in [(glow::VERTEX_SHADER, vs), (glow::FRAGMENT_SHADER, fs)] {
            let sh = gl.create_shader(kind)?;
            gl.shader_source(sh, &format!("{header}{src}"));
            gl.compile_shader(sh);
            if !gl.get_shader_compile_status(sh) {
                let log = gl.get_shader_info_log(sh);
                gl.delete_shader(sh);
                for s in shaders {
                    gl.delete_shader(s);
                }
                gl.delete_program(program);
                return Err(format!("3D shader did not compile: {log}"));
            }
            gl.attach_shader(program, sh);
            shaders.push(sh);
        }
        for (i, name) in attribs.iter().enumerate() {
            gl.bind_attrib_location(program, i as u32, name);
        }
        gl.link_program(program);
        let ok = gl.get_program_link_status(program);
        for s in shaders {
            gl.detach_shader(program, s);
            gl.delete_shader(s);
        }
        if !ok {
            let log = gl.get_program_info_log(program);
            gl.delete_program(program);
            return Err(format!("3D program did not link: {log}"));
        }
        Ok(Prog(program))
    }
}

fn target(gl: &glow::Context, size: (i32, i32), depth: bool, fmt: Format) -> Result<Target, String> {
    let (internal, ty) = match fmt {
        Format::Half => (glow::RGBA16F, glow::HALF_FLOAT),
        Format::Byte => (glow::RGBA8, glow::UNSIGNED_BYTE),
    };
    // SAFETY: a texture, a framebuffer and a renderbuffer made and bound
    // here on the painting context; bindings are put back to none before
    // the return, and the handles are freed by `Target::delete`.
    unsafe {
        let tex = gl.create_texture()?;
        gl.bind_texture(glow::TEXTURE_2D, Some(tex));
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            internal as i32,
            size.0,
            size.1,
            0,
            glow::RGBA,
            ty,
            glow::PixelUnpackData::Slice(None),
        );
        for (p, v) in [
            (glow::TEXTURE_MIN_FILTER, glow::LINEAR),
            (glow::TEXTURE_MAG_FILTER, glow::LINEAR),
            (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
            (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
        ] {
            gl.tex_parameter_i32(glow::TEXTURE_2D, p, v as i32);
        }
        gl.bind_texture(glow::TEXTURE_2D, None);
        let fbo = gl.create_framebuffer()?;
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
        gl.framebuffer_texture_2d(
            glow::FRAMEBUFFER,
            glow::COLOR_ATTACHMENT0,
            glow::TEXTURE_2D,
            Some(tex),
            0,
        );
        let rb = if depth {
            let rb = gl.create_renderbuffer()?;
            gl.bind_renderbuffer(glow::RENDERBUFFER, Some(rb));
            gl.renderbuffer_storage(glow::RENDERBUFFER, glow::DEPTH_COMPONENT16, size.0, size.1);
            gl.framebuffer_renderbuffer(
                glow::FRAMEBUFFER,
                glow::DEPTH_ATTACHMENT,
                glow::RENDERBUFFER,
                Some(rb),
            );
            gl.bind_renderbuffer(glow::RENDERBUFFER, None);
            Some(rb)
        } else {
            None
        };
        let status = gl.check_framebuffer_status(glow::FRAMEBUFFER);
        gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        let t = Target {
            fbo,
            tex,
            depth: rb,
            size,
        };
        if status != glow::FRAMEBUFFER_COMPLETE {
            t.delete(gl);
            return Err(format!("3D framebuffer incomplete ({status:#x})"));
        }
        Ok(t)
    }
}

impl Target {
    fn delete(&self, gl: &glow::Context) {
        // SAFETY: frees handles this module made (module doc).
        unsafe {
            gl.delete_framebuffer(self.fbo);
            gl.delete_texture(self.tex);
            if let Some(d) = self.depth {
                gl.delete_renderbuffer(d);
            }
        }
    }
}

/// The attributes of the vertices, in the order the batch lays them out:
/// name and floats.
const SOLID_ATTRS: [(&str, i32); 7] = [
    ("a_pos", 3),
    ("a_nrm", 3),
    ("a_col", 4),
    ("a_emit", 1),
    ("a_ctr", 3),
    ("a_half", 3),
    ("a_mat", 1),
];
const LINE_ATTRS: [(&str, i32); 8] = [
    ("a_a", 3),
    ("a_b", 3),
    ("a_corner", 2),
    ("a_col", 4),
    ("a_width", 1),
    ("a_emit", 1),
    ("a_phase", 1),
    ("a_flow", 1),
];
const POOL_ATTRS: [(&str, i32); 3] = [("a_pos", 3), ("a_uv", 2), ("a_col", 3)];

fn bytes(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_ne_bytes()).collect()
}

fn names(a: &[(&'static str, i32)]) -> Vec<&'static str> {
    a.iter().map(|x| x.0).collect()
}

/// Whether the context can render to half-float targets: a small one is
/// tried (WebGL2 needs `EXT_color_buffer_float` or `_half_float`, which
/// glow enables when the browser has them).
fn half_float_works(gl: &glow::Context) -> bool {
    match target(gl, (4, 4), false, Format::Half) {
        Ok(t) => {
            t.delete(gl);
            true
        }
        Err(_) => false,
    }
}

impl Res {
    fn new(gl: &glow::Context) -> Result<Self, String> {
        let h = header(gl)?;
        let fmt = if half_float_works(gl) {
            Format::Half
        } else {
            Format::Byte
        };
        let solid = compile(gl, &h, SOLID_VS, SOLID_FS, &names(&SOLID_ATTRS))?;
        let line = compile(gl, &h, LINE_VS, LINE_FS, &names(&LINE_ATTRS))?;
        let pool = compile(gl, &h, POOL_VS, POOL_FS, &names(&POOL_ATTRS))?;
        let bright = compile(gl, &h, QUAD_VS, BRIGHT_FS, &[])?;
        let down = compile(gl, &h, QUAD_VS, DOWN_FS, &[])?;
        let up = compile(gl, &h, QUAD_VS, UP_FS, &[])?;
        let composite = compile(gl, &h, QUAD_VS, COMPOSITE_FS, &[])?;
        // SAFETY: a vertex array and a buffer made on the painting context.
        let (vao, vbo) = unsafe { (gl.create_vertex_array()?, gl.create_buffer()?) };
        Ok(Self {
            fmt,
            vao,
            vbo,
            solid,
            line,
            pool,
            bright,
            down,
            up,
            composite,
            scene: None,
            downs: Vec::new(),
            ups: Vec::new(),
        })
    }

    fn delete(self, gl: &glow::Context) {
        // SAFETY: frees handles this module made (module doc).
        unsafe {
            gl.delete_vertex_array(self.vao);
            gl.delete_buffer(self.vbo);
            for p in [
                &self.solid,
                &self.line,
                &self.pool,
                &self.bright,
                &self.down,
                &self.up,
                &self.composite,
            ] {
                gl.delete_program(p.0);
            }
        }
        for t in self.scene.iter().chain(&self.downs).chain(&self.ups) {
            t.delete(gl);
        }
    }

    /// The targets for a viewport of `size` pixels, made again when it changed.
    fn fit(&mut self, gl: &glow::Context, size: (i32, i32)) -> Result<(), String> {
        let level = |i: usize| ((size.0 >> (i + 1)).max(1), (size.1 >> (i + 1)).max(1));
        let fresh = self.scene.as_ref().is_some_and(|t| t.size == size) && self.downs.len() == LEVELS;
        if fresh {
            return Ok(());
        }
        for t in self.scene.take().iter().chain(&self.downs).chain(&self.ups) {
            t.delete(gl);
        }
        self.downs.clear();
        self.ups.clear();
        self.scene = Some(target(gl, size, true, self.fmt)?);
        for i in 0..LEVELS {
            self.downs.push(target(gl, level(i), false, self.fmt)?);
        }
        for i in 0..LEVELS - 1 {
            self.ups.push(target(gl, level(i), false, self.fmt)?);
        }
        Ok(())
    }

    /// Draws `verts` (a batch's floats) with `prog` as triangles.
    fn geometry(&self, gl: &glow::Context, prog: &Prog, verts: &[f32], attrs: &[(&str, i32)], stride: usize) {
        if verts.is_empty() {
            return;
        }
        // SAFETY: uploads into this module's buffer and draws from it; the
        // attribute layout is the batch's (`SOLID_STRIDE`, `LINE_STRIDE`,
        // `POOL_STRIDE`), and `verts` is a whole number of vertices (the
        // batch's builders push `stride` floats at a time), so the draw
        // reads nothing past the upload.
        unsafe {
            gl.use_program(Some(prog.0));
            gl.bind_vertex_array(Some(self.vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.vbo));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, &bytes(verts), glow::STREAM_DRAW);
            let mut at = 0;
            for (i, (_, n)) in attrs.iter().enumerate() {
                gl.enable_vertex_attrib_array(i as u32);
                gl.vertex_attrib_pointer_f32(i as u32, *n, glow::FLOAT, false, (stride * 4) as i32, at * 4);
                at += n;
            }
            gl.draw_arrays(glow::TRIANGLES, 0, (verts.len() / stride) as i32);
            for i in 0..attrs.len() {
                gl.disable_vertex_attrib_array(i as u32);
            }
        }
    }

    /// The uniforms the scene programs share.
    fn set_scene(&self, gl: &glow::Context, f: &Frame, res: (f32, f32)) {
        // SAFETY: uniforms of programs of this module (the location is
        // `None` where a program has no such uniform, which GL ignores).
        unsafe {
            for p in [&self.solid, &self.line, &self.pool] {
                gl.use_program(Some(p.0));
                let at = |n: &str| gl.get_uniform_location(p.0, n);
                gl.uniform_matrix_4_f32_slice(at("u_vp").as_ref(), false, &f.vp);
                gl.uniform_2_f32(at("u_res").as_ref(), res.0, res.1);
                gl.uniform_1_f32(at("u_ppp").as_ref(), f.ppp);
                gl.uniform_1_f32(at("u_time").as_ref(), f.time);
                gl.uniform_3_f32(at("u_eye").as_ref(), f.eye[0], f.eye[1], f.eye[2]);
                gl.uniform_4_f32(at("u_fog").as_ref(), f.fog[0], f.fog[1], f.fog[2], f.fog[3]);
            }
        }
    }

    /// The scene into the target bound now: opaque blocks, the light on
    /// the floor, then what blends (slabs, wires, the ghosts of a dive).
    fn scene_pass(&self, gl: &glow::Context, f: &Frame) {
        // SAFETY: state calls on the painting context; egui's are
        // re-applied after the callback (module doc).
        unsafe {
            gl.disable(glow::BLEND);
            gl.enable(glow::DEPTH_TEST);
            gl.depth_func(glow::LEQUAL);
            gl.depth_mask(true);
        }
        self.geometry(gl, &self.solid, &f.batch.solids, &SOLID_ATTRS, SOLID_STRIDE);
        // SAFETY: as above.
        unsafe {
            gl.enable(glow::BLEND);
            gl.blend_func_separate(glow::ONE, glow::ONE, glow::ZERO, glow::ONE);
            gl.depth_mask(false);
        }
        self.geometry(gl, &self.pool, &f.batch.pools, &POOL_ATTRS, POOL_STRIDE);
        // SAFETY: as above.
        unsafe {
            gl.blend_func_separate(
                glow::ONE,
                glow::ONE_MINUS_SRC_ALPHA,
                glow::ONE,
                glow::ONE_MINUS_SRC_ALPHA,
            );
        }
        self.geometry(gl, &self.solid, &f.batch.solids_ghost, &SOLID_ATTRS, SOLID_STRIDE);
        self.geometry(gl, &self.line, &f.batch.lines, &LINE_ATTRS, LINE_STRIDE);
        self.geometry(gl, &self.line, &f.batch.lines_ghost, &LINE_ATTRS, LINE_STRIDE);
        // SAFETY: as above.
        unsafe {
            gl.depth_mask(true);
            gl.disable(glow::BLEND);
        }
    }

    /// A full-screen pass of `prog` into `to` (or the screen) over `view`,
    /// reading the textures in `from` on units 0, 1, ...
    fn quad(
        &self,
        gl: &glow::Context,
        prog: &Prog,
        to: Option<glow::Framebuffer>,
        view: (i32, i32, i32, i32),
        from: &[glow::Texture],
        set: impl FnOnce(&glow::Context, &glow::Program),
    ) {
        // SAFETY: binds this module's framebuffers and textures and draws
        // three vertices with no attributes; the textures are unbound
        // again before the return.
        unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, to);
            gl.viewport(view.0, view.1, view.2, view.3);
            gl.disable(glow::DEPTH_TEST);
            gl.use_program(Some(prog.0));
            for (i, t) in from.iter().enumerate() {
                gl.active_texture(glow::TEXTURE0 + i as u32);
                gl.bind_texture(glow::TEXTURE_2D, Some(*t));
            }
            set(gl, &prog.0);
            gl.bind_vertex_array(Some(self.vao));
            gl.draw_arrays(glow::TRIANGLES, 0, 3);
            for i in (0..from.len()).rev() {
                gl.active_texture(glow::TEXTURE0 + i as u32);
                gl.bind_texture(glow::TEXTURE_2D, None);
            }
        }
    }

    fn draw(
        &mut self,
        gl: &glow::Context,
        f: &Frame,
        size: (i32, i32),
        at: (i32, i32),
        screen: Option<glow::Framebuffer>,
    ) -> Result<(), String> {
        self.fit(gl, size)?;
        let Some(scene) = &self.scene else { return Ok(()) };
        // SAFETY: clears the target bound just before; scissor is off for
        // the offscreen passes (its box is the window's, not theirs).
        unsafe {
            gl.disable(glow::SCISSOR_TEST);
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(scene.fbo));
            gl.viewport(0, 0, size.0, size.1);
            gl.clear_color(0.0, 0.0, 0.0, 0.0);
            gl.clear_depth_f32(1.0);
            gl.clear(glow::COLOR_BUFFER_BIT | glow::DEPTH_BUFFER_BIT);
        }
        self.set_scene(gl, f, (size.0 as f32, size.1 as f32));
        self.scene_pass(gl, f);
        let hdr = self.fmt == Format::Half;
        // Half floats keep what is brighter than white; bytes cannot, so
        // the threshold sits lower there.
        let thr = if hdr { 0.9 } else { 0.7 };
        let strength = if f.batch.any_lit { f.bloom } else { 0.0 };
        if strength > 0.0 {
            let lvl = |i: usize| (self.downs[i].size.0, self.downs[i].size.1);
            let texel = |s: (i32, i32)| (1.0 / s.0 as f32, 1.0 / s.1 as f32);
            // The way down.
            let t0 = texel(size);
            self.quad(
                gl,
                &self.bright,
                Some(self.downs[0].fbo),
                (0, 0, lvl(0).0, lvl(0).1),
                &[scene.tex],
                |gl, p| {
                    // SAFETY: uniforms of the bright pass, in use.
                    unsafe {
                        gl.uniform_1_i32(gl.get_uniform_location(*p, "u_tex").as_ref(), 0);
                        gl.uniform_2_f32(gl.get_uniform_location(*p, "u_texel").as_ref(), t0.0, t0.1);
                        gl.uniform_1_f32(gl.get_uniform_location(*p, "u_thr").as_ref(), thr);
                    }
                },
            );
            for i in 1..LEVELS {
                let t = texel(lvl(i - 1));
                self.quad(
                    gl,
                    &self.down,
                    Some(self.downs[i].fbo),
                    (0, 0, lvl(i).0, lvl(i).1),
                    &[self.downs[i - 1].tex],
                    |gl, p| {
                        // SAFETY: uniforms of the down pass, in use.
                        unsafe {
                            gl.uniform_1_i32(gl.get_uniform_location(*p, "u_tex").as_ref(), 0);
                            gl.uniform_2_f32(gl.get_uniform_location(*p, "u_texel").as_ref(), t.0, t.1);
                        }
                    },
                );
            }
            // The way up: each level takes the widened sum of those below.
            for i in (0..LEVELS - 1).rev() {
                let lo = if i == LEVELS - 2 {
                    &self.downs[i + 1]
                } else {
                    &self.ups[i + 1]
                };
                let t = texel((lo.size.0, lo.size.1));
                self.quad(
                    gl,
                    &self.up,
                    Some(self.ups[i].fbo),
                    (0, 0, lvl(i).0, lvl(i).1),
                    &[self.downs[i].tex, lo.tex],
                    |gl, p| {
                        // SAFETY: uniforms of the up pass, in use.
                        unsafe {
                            gl.uniform_1_i32(gl.get_uniform_location(*p, "u_base").as_ref(), 0);
                            gl.uniform_1_i32(gl.get_uniform_location(*p, "u_lo").as_ref(), 1);
                            gl.uniform_2_f32(gl.get_uniform_location(*p, "u_texel").as_ref(), t.0, t.1);
                            gl.uniform_1_f32(gl.get_uniform_location(*p, "u_k").as_ref(), 0.65);
                        }
                    },
                );
            }
        }
        // The composite onto egui's framebuffer, in the callback's
        // rectangle, with the scissor egui set for it.
        // SAFETY: back to egui's target; the scissor test is on again, as
        // egui had it.
        unsafe {
            gl.enable(glow::SCISSOR_TEST);
        }
        let bloom_tex = if strength > 0.0 {
            self.ups[0].tex
        } else {
            scene.tex
        };
        self.quad(
            gl,
            &self.composite,
            screen,
            (at.0, at.1, size.0, size.1),
            &[scene.tex, bloom_tex],
            |gl, p| {
                // SAFETY: uniforms of the composite program, in use.
                unsafe {
                    gl.uniform_1_i32(gl.get_uniform_location(*p, "u_scene").as_ref(), 0);
                    gl.uniform_1_i32(gl.get_uniform_location(*p, "u_bloom").as_ref(), 1);
                    gl.uniform_1_f32(gl.get_uniform_location(*p, "u_bloom_k").as_ref(), strength);
                    gl.uniform_1_f32(gl.get_uniform_location(*p, "u_exposure").as_ref(), f.exposure);
                    gl.uniform_3_f32(
                        gl.get_uniform_location(*p, "u_bg").as_ref(),
                        f.bg[0],
                        f.bg[1],
                        f.bg[2],
                    );
                }
            },
        );
        // SAFETY: puts back what egui does not re-apply itself.
        unsafe {
            gl.active_texture(glow::TEXTURE0);
            gl.bind_vertex_array(None);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
            gl.depth_mask(true);
            gl.disable(glow::DEPTH_TEST);
        }
        Ok(())
    }
}
