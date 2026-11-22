//! The 3D canvas's GL pass, inside egui's paint callback on the glow
//! backend (`egui_glow::CallbackFn`): one program for shapes (rounded
//! tiles, shadows, lamp glass, halos), one for wires, drawn far to near
//! straight into egui's framebuffer. No depth buffer, no offscreen
//! target, no extension: it runs the same on a desktop GPU, on
//! integrated graphics and on software WebGL2.
//!
//! This is the one module of the canvas that holds `unsafe`: glow's calls
//! are `unsafe fn`s (GL's state is global and a bad handle is the caller's
//! to avoid), and the card asks for our own GL pass (Nick's ruling,
//! 2026-11-21). Every call is in a block with its safety comment, and the
//! crate denies `unsafe_code` everywhere else (main.rs). The contract for
//! all of them is the same:
//!
//! - the context is the one `egui_glow::Painter` hands the callback, on
//!   the thread that is painting, with its context current;
//! - every handle used was made by this module from that context and is
//!   deleted only by [`Shared::destroy`], which the app calls on exit and
//!   when the view is switched off;
//! - what egui expects afterwards is put back: the buffer and vertex
//!   array bindings. egui's own `prepare_painting` re-applies the rest
//!   (program, blend function, viewport, active texture) after every
//!   callback, and the scissor egui set for the callback stays on.
//!
//! The shaders take their version line from `egui_glow`'s `ShaderVersion`,
//! so they compile as GLSL 1.40 on a desktop and as GLSL ES 3.00 in the
//! browser (WebGL2). A context older than that (GLSL 1.20, ES 1.00, which
//! is `WebGL1`) has no `in`/`out`: the pass reports it and the view stays
//! 2D.
#![allow(unsafe_code)]
// GL takes sizes as i32 and enums as u32; ours are small and known.
#![allow(clippy::cast_possible_wrap)]

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use eframe::egui::PaintCallbackInfo;
use eframe::egui_glow::{Painter, ShaderVersion};
use eframe::glow::{self, HasContext as _};

use crate::batch3d::{Batch, LINE_STRIDE, SHAPE_STRIDE};
use crate::cam3d::M4;

/// What one frame draws.
pub struct Frame {
    pub vp: M4,
    pub batch: Batch,
    /// The look's accent, for a live tile's edge.
    pub accent: [f32; 3],
    /// The page is dark: shapes are dithered for it.
    pub dark: bool,
    /// The clock the pulses run on, in wavelengths.
    pub time: f32,
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
        if let Some(r) = res.as_ref() {
            r.draw(gl, frame, (size.0 as f32, size.1 as f32));
        }
    }
}

const SHAPE_VS: &str = "
uniform mat4 u_vp;
in vec3 a_pos;
in vec2 a_uv;
in vec2 a_half;
in float a_rad;
in vec4 a_col;
in vec4 a_par;
out vec2 v_uv;
out vec2 v_half;
out float v_rad;
out vec4 v_col;
out vec4 v_par;
void main() {
    v_uv = a_uv;
    v_half = a_half;
    v_rad = a_rad;
    v_col = a_col;
    v_par = a_par;
    gl_Position = u_vp * vec4(a_pos, 1.0);
}
";

const SHAPE_FS: &str = "
uniform vec3 u_accent;
uniform float u_dark;
in vec2 v_uv;
in vec2 v_half;
in float v_rad;
in vec4 v_col;
in vec4 v_par;
out vec4 o_col;
float hash(vec2 p) {
    return fract(sin(dot(p, vec2(12.9898, 78.233))) * 43758.5453);
}
// Signed distance to a rounded rectangle of half size h and radius r.
float rr(vec2 p, vec2 h, float r) {
    r = min(r, min(h.x, h.y));
    vec2 q = abs(p) - h + r;
    return length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - r;
}
void main() {
    int mode = int(v_par.x + 0.5);
    vec4 outc = vec4(0.0);
    float dither = (hash(gl_FragCoord.xy) - 0.5) / 255.0;
    if (mode == 0 || mode == 4) {
        float d = rr(v_uv, v_half, v_rad);
        float aa = fwidth(d) * 0.75 + 1e-5;
        float cov = 1.0 - smoothstep(-aa, aa, d);
        vec3 c = v_col.rgb;
        if (mode == 0 && v_par.y > 0.5) {
            // The top of a tile: a gentle light from the upper left, a
            // thin lighter rim, and when it is live a lit edge and a soft
            // glow inside it.
            vec2 q = v_uv / max(v_half, vec2(0.001));
            c *= 1.0 + 0.07 * (-(q.x + q.y) * 0.5);
            float rim = smoothstep(-aa * 2.5, -aa * 0.5, d) * (1.0 - smoothstep(-aa * 0.5, aa * 0.5, d));
            c += (u_dark > 0.5 ? 0.05 : -0.03) * rim;
            float edge = (1.0 - smoothstep(0.0, 0.06 + aa, -d));
            c = mix(c, u_accent, v_par.z * (0.8 * edge + 0.1 * exp(d / 0.4)));
            c += v_par.w * 0.14 * exp(-dot(q + vec2(0.35, 0.35), q + vec2(0.35, 0.35)) * 5.0);
        } else if (mode == 4) {
            // The board: a faint dot grid and a lighter edge.
            vec2 g = fract(v_uv) - 0.5;
            float dots = 1.0 - smoothstep(0.03, 0.03 + fwidth(v_uv.x) * 1.2, length(g));
            float inside = smoothstep(0.0, 0.6, -d);
            c += (u_dark > 0.5 ? 0.06 : -0.07) * dots * v_par.w * inside;
            float edge = (1.0 - smoothstep(0.0, 0.05 + aa, -d));
            c += (u_dark > 0.5 ? 0.05 : -0.04) * edge;
        }
        float a = cov * v_col.a;
        outc = vec4(c * a + dither * a, a);
    } else if (mode == 1) {
        // A shadow: a rounded rectangle, blurred by distance.
        float d = rr(v_uv, v_half, v_rad);
        float s = max(v_par.y, 0.01);
        float a = v_par.z * (1.0 - smoothstep(-s, s * 1.6, d));
        outc = vec4(0.0, 0.0, 0.0, a * a * 0.6 + a * 0.4);
        outc.rgb = vec3(0.0);
    } else if (mode == 2) {
        // A halo: soft and round.
        float r = length(v_uv);
        float f = exp(-r * r * 3.2) * (1.0 - smoothstep(0.7, 1.0, r));
        outc = vec4(v_col.rgb * f * v_par.z, 0.0);
    } else if (mode == 5) {
        // The light a lamp or a pressed button puts on the board.
        vec2 q = v_uv / max(v_half + vec2(v_rad * 0.0), vec2(0.001));
        float f = exp(-dot(q, q) * 1.6);
        f *= 1.0 - smoothstep(0.8, 1.0, length(v_uv / (v_half + 1.0)));
        outc = vec4(v_col.rgb * f * v_par.z, 0.0);
    } else {
        // Glass: a sphere lit from the upper left, dark when off, a
        // bright core and a soft highlight when on.
        float r = length(v_uv);
        float aa = fwidth(r) * 0.9 + 1e-5;
        float cov = 1.0 - smoothstep(1.0 - aa, 1.0 + aa, r);
        float z = sqrt(max(1.0 - r * r, 0.0));
        vec3 n = normalize(vec3(v_uv, z));
        vec3 l = normalize(vec3(-0.4, 0.6, 0.7));
        float diff = max(dot(n, l), 0.0);
        float spec = pow(max(dot(reflect(-l, n), vec3(0.0, 0.0, 1.0)), 0.0), 36.0);
        float rimf = pow(1.0 - z, 2.2);
        vec3 hue = v_col.rgb;
        vec3 off = u_dark > 0.5 ? hue * (0.16 + 0.22 * diff) + vec3(0.04) * rimf : mix(vec3(0.86), hue, 0.4) * (0.72 + 0.28 * diff);
        float core = pow(z, 1.6);
        vec3 on = hue * (0.72 + 0.5 * core) + vec3(1.0) * core * core * 0.22 + hue * rimf * 0.25;
        vec3 c = mix(off, on, v_par.z) + vec3(spec) * (0.45 + 0.35 * v_par.z);
        float a = cov * v_col.a;
        outc = vec4(c * a + dither * a, a);
    }
    o_col = outc;
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
out float v_across;
out float v_along;
out float v_flow;
out float v_w;
void main() {
    vec4 ca = u_vp * vec4(a_a, 1.0);
    vec4 cb = u_vp * vec4(a_b, 1.0);
    float near = 0.05;
    v_col = a_col;
    v_flow = a_flow;
    v_along = a_phase + a_corner.x * length(a_b - a_a);
    v_w = max(a_width * u_ppp, 1.5);
    v_across = a_corner.y;
    if (ca.w < near && cb.w < near) {
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
    // Rounded ends: the quad runs half a width past each end.
    vec2 off = (nrm * a_corner.y * half_w + dir * (a_corner.x * 2.0 - 1.0) * half_w) * 2.0 / u_res;
    v_across = a_corner.y * half_w / (v_w * 0.5);
    gl_Position = vec4(c.xy + off * c.w, c.z, c.w);
}
";

const LINE_FS: &str = "
uniform float u_time;
uniform float u_soft;
in vec4 v_col;
in float v_across;
in float v_along;
in float v_flow;
in float v_w;
out vec4 o_col;
void main() {
    float d = abs(v_across);
    float cov = u_soft > 0.5 ? exp(-d * d * 2.2) : 1.0 - smoothstep(1.0 - 2.0 / v_w, 1.0 + 1.0 / v_w, d);
    // A soft pulse travels with the data.
    float ph = fract(v_along / 4.5 - u_time);
    float bump = 0.5 + 0.5 * cos((ph - 0.5) * 6.2831853);
    float lift = v_flow * 0.2 * bump * bump;
    vec3 rgb = v_col.rgb * (1.0 + lift) + vec3(lift * 0.25);
    float a = cov * v_col.a;
    o_col = u_soft > 0.5 ? vec4(rgb * a, 0.0) : vec4(rgb * a, a);
}
";

/// A linked program.
struct Prog(glow::Program);

struct Res {
    vao: glow::VertexArray,
    vbo: glow::Buffer,
    shape: Prog,
    line: Prog,
}

fn header(gl: &glow::Context) -> Result<String, String> {
    // The one GL query (a string), on the painting context.
    let v = ShaderVersion::get(gl);
    if !v.is_new_shader_interface() {
        return Err("3D needs WebGL2 or OpenGL 3: this context is older".into());
    }
    let mut h = v.version_declaration().to_string();
    if v.is_embedded() {
        h.push_str("precision highp float;\n");
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

/// The attributes of the vertices, in the order the batch lays them out:
/// name and floats.
const SHAPE_ATTRS: [(&str, i32); 6] = [
    ("a_pos", 3),
    ("a_uv", 2),
    ("a_half", 2),
    ("a_rad", 1),
    ("a_col", 4),
    ("a_par", 4),
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

fn bytes(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_ne_bytes()).collect()
}

fn names(a: &[(&'static str, i32)]) -> Vec<&'static str> {
    a.iter().map(|x| x.0).collect()
}

impl Res {
    fn new(gl: &glow::Context) -> Result<Self, String> {
        let h = header(gl)?;
        let shape = compile(gl, &h, SHAPE_VS, SHAPE_FS, &names(&SHAPE_ATTRS))?;
        let line = compile(gl, &h, LINE_VS, LINE_FS, &names(&LINE_ATTRS))?;
        // SAFETY: a vertex array and a buffer made on the painting context.
        let (vao, vbo) = unsafe { (gl.create_vertex_array()?, gl.create_buffer()?) };
        Ok(Self {
            vao,
            vbo,
            shape,
            line,
        })
    }

    fn delete(self, gl: &glow::Context) {
        // SAFETY: frees handles this module made (module doc).
        unsafe {
            gl.delete_vertex_array(self.vao);
            gl.delete_buffer(self.vbo);
            gl.delete_program(self.shape.0);
            gl.delete_program(self.line.0);
        }
    }

    /// Draws `verts` (a batch's floats) with `prog` as triangles.
    fn geometry(&self, gl: &glow::Context, prog: &Prog, verts: &[f32], attrs: &[(&str, i32)], stride: usize) {
        if verts.is_empty() {
            return;
        }
        // SAFETY: uploads into this module's buffer and draws from it; the
        // attribute layout is the batch's (`SHAPE_STRIDE`, `LINE_STRIDE`),
        // and `verts` is a whole number of vertices (the batch's builders
        // push `stride` floats at a time), so the draw reads nothing past
        // the upload.
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

    fn shapes(&self, gl: &glow::Context, v: &[f32]) {
        self.geometry(gl, &self.shape, v, &SHAPE_ATTRS, SHAPE_STRIDE);
    }

    fn lines(&self, gl: &glow::Context, v: &[f32], soft: bool) {
        // SAFETY: a uniform of the line program, which is made current
        // first.
        unsafe {
            gl.use_program(Some(self.line.0));
            gl.uniform_1_f32(
                gl.get_uniform_location(self.line.0, "u_soft").as_ref(),
                f32::from(u8::from(soft)),
            );
        }
        self.geometry(gl, &self.line, v, &LINE_ATTRS, LINE_STRIDE);
    }

    fn draw(&self, gl: &glow::Context, f: &Frame, res: (f32, f32)) {
        // SAFETY: uniforms of programs of this module, and the blend state
        // of the shapes (premultiplied alpha, as egui blends); no depth
        // test, as egui has it.
        unsafe {
            gl.use_program(Some(self.shape.0));
            gl.uniform_matrix_4_f32_slice(
                gl.get_uniform_location(self.shape.0, "u_vp").as_ref(),
                false,
                &f.vp,
            );
            gl.uniform_3_f32(
                gl.get_uniform_location(self.shape.0, "u_accent").as_ref(),
                f.accent[0],
                f.accent[1],
                f.accent[2],
            );
            gl.uniform_1_f32(
                gl.get_uniform_location(self.shape.0, "u_dark").as_ref(),
                f32::from(u8::from(f.dark)),
            );
            gl.use_program(Some(self.line.0));
            gl.uniform_matrix_4_f32_slice(
                gl.get_uniform_location(self.line.0, "u_vp").as_ref(),
                false,
                &f.vp,
            );
            gl.uniform_2_f32(
                gl.get_uniform_location(self.line.0, "u_res").as_ref(),
                res.0,
                res.1,
            );
            gl.uniform_1_f32(gl.get_uniform_location(self.line.0, "u_ppp").as_ref(), f.ppp);
            gl.uniform_1_f32(gl.get_uniform_location(self.line.0, "u_time").as_ref(), f.time);
            gl.enable(glow::BLEND);
            gl.blend_func_separate(
                glow::ONE,
                glow::ONE_MINUS_SRC_ALPHA,
                glow::ONE,
                glow::ONE_MINUS_SRC_ALPHA,
            );
            gl.disable(glow::DEPTH_TEST);
        }
        for b in &f.batch.boards {
            self.shapes(gl, &b.under);
            self.lines(gl, &b.lines, false);
            self.shapes(gl, &b.over);
        }
        self.lines(gl, &f.batch.links, false);
        // The glows add light: halos, and the soft width round a live wire.
        // SAFETY: the blend function for adding.
        unsafe {
            gl.blend_func_separate(glow::ONE, glow::ONE, glow::ZERO, glow::ONE);
        }
        self.lines(gl, &f.batch.glow_lines, true);
        self.shapes(gl, &f.batch.glow);
        // SAFETY: puts back what egui does not re-apply itself.
        unsafe {
            gl.bind_vertex_array(None);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
        }
    }
}
