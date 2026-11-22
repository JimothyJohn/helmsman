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

use crate::batch3d::{Batch, LINE_STRIDE, SOLID_STRIDE};
use crate::cam3d::M4;

/// What one frame draws.
pub struct Frame {
    pub vp: M4,
    pub batch: Batch,
    pub bg: [f32; 3],
    pub bloom: bool,
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
out vec3 v_nrm;
out vec4 v_col;
out float v_emit;
void main() {
    v_nrm = a_nrm;
    v_col = a_col;
    v_emit = a_emit;
    gl_Position = u_vp * vec4(a_pos, 1.0);
}
";

const SOLID_FS: &str = "
uniform float u_bright;
in vec3 v_nrm;
in vec4 v_col;
in float v_emit;
out vec4 o_col;
void main() {
    if (u_bright > 0.5) {
        o_col = v_emit > 0.5 ? vec4(v_col.rgb, 1.0) : vec4(0.0, 0.0, 0.0, 1.0);
        return;
    }
    vec3 n = normalize(v_nrm);
    float l = 0.42 + 0.58 * max(dot(n, normalize(vec3(-0.3, 0.9, 0.5))), 0.0);
    vec3 c = v_col.rgb * l + v_col.rgb * v_emit * 0.35;
    o_col = vec4(c, v_col.a);
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
out vec4 v_col;
out float v_emit;
void main() {
    vec4 ca = u_vp * vec4(a_a, 1.0);
    vec4 cb = u_vp * vec4(a_b, 1.0);
    float near = 0.05;
    v_col = a_col;
    v_emit = a_emit;
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
    vec2 off = (nrm * a_corner.y + dir * (a_corner.x * 2.0 - 1.0)) * a_width * u_ppp * 0.5 * 2.0 / u_res;
    gl_Position = vec4(c.xy + off * c.w, c.z - 0.0004 * c.w, c.w);
}
";

const LINE_FS: &str = "
uniform float u_bright;
in vec4 v_col;
in float v_emit;
out vec4 o_col;
void main() {
    if (u_bright > 0.5) {
        o_col = v_emit > 0.5 ? vec4(v_col.rgb, 1.0) : vec4(0.0, 0.0, 0.0, 1.0);
        return;
    }
    o_col = vec4(v_col.rgb + v_col.rgb * v_emit * 0.35, v_col.a);
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

const BLUR_FS: &str = "
uniform sampler2D u_tex;
uniform vec2 u_step;
in vec2 v_uv;
out vec4 o_col;
void main() {
    vec3 c = texture(u_tex, v_uv).rgb * 0.227027;
    c += texture(u_tex, v_uv + u_step * 1.0).rgb * 0.1945946;
    c += texture(u_tex, v_uv - u_step * 1.0).rgb * 0.1945946;
    c += texture(u_tex, v_uv + u_step * 2.0).rgb * 0.1216216;
    c += texture(u_tex, v_uv - u_step * 2.0).rgb * 0.1216216;
    c += texture(u_tex, v_uv + u_step * 3.0).rgb * 0.054054;
    c += texture(u_tex, v_uv - u_step * 3.0).rgb * 0.054054;
    c += texture(u_tex, v_uv + u_step * 4.0).rgb * 0.016216;
    c += texture(u_tex, v_uv - u_step * 4.0).rgb * 0.016216;
    o_col = vec4(c, 1.0);
}
";

const COMPOSITE_FS: &str = "
uniform sampler2D u_scene;
uniform sampler2D u_bloom;
uniform float u_strength;
in vec2 v_uv;
out vec4 o_col;
void main() {
    vec3 s = texture(u_scene, v_uv).rgb;
    vec3 b = texture(u_bloom, v_uv).rgb;
    o_col = vec4(s + b * u_strength, 1.0);
}
";

/// A linked program.
struct Prog(glow::Program);

/// An offscreen target: colour texture, optional depth.
struct Target {
    fbo: glow::Framebuffer,
    tex: glow::Texture,
    depth: Option<glow::Renderbuffer>,
    size: (i32, i32),
}

struct Res {
    vao: glow::VertexArray,
    vbo: glow::Buffer,
    solid: Prog,
    line: Prog,
    blur: Prog,
    composite: Prog,
    scene: Option<Target>,
    /// Half size: the bright pass, then the blur ping-pong.
    glow_a: Option<Target>,
    glow_b: Option<Target>,
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

fn target(gl: &glow::Context, size: (i32, i32), depth: bool) -> Result<Target, String> {
    // SAFETY: a texture, a framebuffer and a renderbuffer made and bound
    // here on the painting context; bindings are put back to none before
    // the return, and the handles are freed by `Target::delete`.
    unsafe {
        let tex = gl.create_texture()?;
        gl.bind_texture(glow::TEXTURE_2D, Some(tex));
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::RGBA8 as i32,
            size.0,
            size.1,
            0,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
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

/// The attributes of the solid and the line vertex: name and floats.
const SOLID_ATTRS: [(&str, i32); 4] = [("a_pos", 3), ("a_nrm", 3), ("a_col", 4), ("a_emit", 1)];
const LINE_ATTRS: [(&str, i32); 6] = [
    ("a_a", 3),
    ("a_b", 3),
    ("a_corner", 2),
    ("a_col", 4),
    ("a_width", 1),
    ("a_emit", 1),
];

fn bytes(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_ne_bytes()).collect()
}

impl Res {
    fn new(gl: &glow::Context) -> Result<Self, String> {
        let h = header(gl)?;
        let names = |a: &[(&'static str, i32)]| a.iter().map(|x| x.0).collect::<Vec<_>>();
        let solid = compile(gl, &h, SOLID_VS, SOLID_FS, &names(&SOLID_ATTRS))?;
        let line = compile(gl, &h, LINE_VS, LINE_FS, &names(&LINE_ATTRS))?;
        let blur = compile(gl, &h, QUAD_VS, BLUR_FS, &[])?;
        let composite = compile(gl, &h, QUAD_VS, COMPOSITE_FS, &[])?;
        // SAFETY: a vertex array and a buffer made on the painting context.
        let (vao, vbo) = unsafe { (gl.create_vertex_array()?, gl.create_buffer()?) };
        Ok(Self {
            vao,
            vbo,
            solid,
            line,
            blur,
            composite,
            scene: None,
            glow_a: None,
            glow_b: None,
        })
    }

    fn delete(self, gl: &glow::Context) {
        // SAFETY: frees handles this module made (module doc).
        unsafe {
            gl.delete_vertex_array(self.vao);
            gl.delete_buffer(self.vbo);
            for p in [&self.solid, &self.line, &self.blur, &self.composite] {
                gl.delete_program(p.0);
            }
        }
        for t in [&self.scene, &self.glow_a, &self.glow_b].into_iter().flatten() {
            t.delete(gl);
        }
    }

    /// The targets for a viewport of `size` pixels, made again when it changed.
    fn fit(&mut self, gl: &glow::Context, size: (i32, i32)) -> Result<(), String> {
        let half = ((size.0 / 2).max(1), (size.1 / 2).max(1));
        let stale = |t: &Option<Target>, want: (i32, i32)| t.as_ref().is_none_or(|t| t.size != want);
        if stale(&self.scene, size) || stale(&self.glow_a, half) || stale(&self.glow_b, half) {
            for t in [self.scene.take(), self.glow_a.take(), self.glow_b.take()]
                .into_iter()
                .flatten()
            {
                t.delete(gl);
            }
            self.scene = Some(target(gl, size, true)?);
            self.glow_a = Some(target(gl, half, true)?);
            self.glow_b = Some(target(gl, half, false)?);
        }
        Ok(())
    }

    /// Draws `verts` (a batch's floats) with `prog` as triangles or as
    /// the line quads.
    fn geometry(&self, gl: &glow::Context, prog: &Prog, verts: &[f32], attrs: &[(&str, i32)], stride: usize) {
        if verts.is_empty() {
            return;
        }
        // SAFETY: uploads into this module's buffer and draws from it; the
        // attribute layout is the batch's (`SOLID_STRIDE`, `LINE_STRIDE`),
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

    fn set_view(gl: &glow::Context, prog: &Prog, vp: &M4, res: (f32, f32), ppp: f32, bright: bool) {
        // SAFETY: uniforms of a program of this module, which is in use.
        unsafe {
            gl.use_program(Some(prog.0));
            gl.uniform_matrix_4_f32_slice(gl.get_uniform_location(prog.0, "u_vp").as_ref(), false, vp);
            gl.uniform_2_f32(gl.get_uniform_location(prog.0, "u_res").as_ref(), res.0, res.1);
            gl.uniform_1_f32(
                gl.get_uniform_location(prog.0, "u_bright").as_ref(),
                f32::from(u8::from(bright)),
            );
            gl.uniform_1_f32(gl.get_uniform_location(prog.0, "u_ppp").as_ref(), ppp);
        }
    }

    /// Scene geometry in the target bound now: opaque, then the ghosts.
    fn scene_pass(&self, gl: &glow::Context, f: &Frame, res: (f32, f32), bright: bool) {
        let ppp = if bright { f.ppp * 0.5 } else { f.ppp };
        Self::set_view(gl, &self.solid, &f.vp, res, ppp, bright);
        Self::set_view(gl, &self.line, &f.vp, res, ppp, bright);
        // SAFETY: state calls on the painting context; egui's are
        // re-applied after the callback (module doc).
        unsafe {
            gl.disable(glow::BLEND);
            gl.enable(glow::DEPTH_TEST);
            gl.depth_func(glow::LEQUAL);
            gl.depth_mask(true);
        }
        self.geometry(gl, &self.solid, &f.batch.solids, &SOLID_ATTRS, SOLID_STRIDE);
        self.geometry(gl, &self.line, &f.batch.lines, &LINE_ATTRS, LINE_STRIDE);
        if bright {
            return;
        }
        // SAFETY: as above.
        unsafe {
            gl.enable(glow::BLEND);
            gl.blend_func(glow::SRC_ALPHA, glow::ONE_MINUS_SRC_ALPHA);
            gl.depth_mask(false);
        }
        self.geometry(gl, &self.solid, &f.batch.solids_ghost, &SOLID_ATTRS, SOLID_STRIDE);
        self.geometry(gl, &self.line, &f.batch.lines_ghost, &LINE_ATTRS, LINE_STRIDE);
        // SAFETY: as above.
        unsafe {
            gl.depth_mask(true);
            gl.disable(glow::BLEND);
        }
    }

    /// A full-screen pass of `prog` into `to` (or the screen), reading the
    /// textures in `from`, on units 0, 1, ...
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
        let (Some(scene), Some(ga), Some(gb)) = (&self.scene, &self.glow_a, &self.glow_b) else {
            return Ok(());
        };
        let half = ga.size;
        // SAFETY: clears the targets bound just before; scissor is off
        // for the offscreen passes (its box is the window's, not theirs).
        unsafe {
            gl.disable(glow::SCISSOR_TEST);
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(scene.fbo));
            gl.viewport(0, 0, size.0, size.1);
            gl.clear_color(f.bg[0], f.bg[1], f.bg[2], 1.0);
            gl.clear_depth_f32(1.0);
            gl.clear(glow::COLOR_BUFFER_BIT | glow::DEPTH_BUFFER_BIT);
        }
        self.scene_pass(gl, f, (size.0 as f32, size.1 as f32), false);
        let glowing = f.bloom && f.batch.any_lit;
        if glowing {
            // Bright pass: only what is lit, the rest black but still in
            // front of what is behind it.
            // SAFETY: as the scene clear above, on the half-size target.
            unsafe {
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(ga.fbo));
                gl.viewport(0, 0, half.0, half.1);
                gl.clear_color(0.0, 0.0, 0.0, 1.0);
                gl.clear_depth_f32(1.0);
                gl.clear(glow::COLOR_BUFFER_BIT | glow::DEPTH_BUFFER_BIT);
            }
            self.scene_pass(gl, f, (half.0 as f32, half.1 as f32), true);
            let step = |dx: f32, dy: f32| {
                move |gl: &glow::Context, p: &glow::Program| {
                    // SAFETY: uniforms of the blur program, in use.
                    unsafe {
                        gl.uniform_1_i32(gl.get_uniform_location(*p, "u_tex").as_ref(), 0);
                        gl.uniform_2_f32(gl.get_uniform_location(*p, "u_step").as_ref(), dx, dy);
                    }
                }
            };
            let (tx, ty) = (1.0 / half.0 as f32, 1.0 / half.1 as f32);
            for _ in 0..2 {
                self.quad(
                    gl,
                    &self.blur,
                    Some(gb.fbo),
                    (0, 0, half.0, half.1),
                    &[ga.tex],
                    step(tx, 0.0),
                );
                self.quad(
                    gl,
                    &self.blur,
                    Some(ga.fbo),
                    (0, 0, half.0, half.1),
                    &[gb.tex],
                    step(0.0, ty),
                );
            }
        }
        // The composite onto egui's framebuffer, in the callback's
        // rectangle, with the scissor egui set for it.
        let strength = if glowing { 1.1 } else { 0.0 };
        // SAFETY: back to egui's target and viewport; the scissor test is
        // on again, as egui had it.
        unsafe {
            gl.enable(glow::SCISSOR_TEST);
        }
        self.quad(
            gl,
            &self.composite,
            screen,
            (at.0, at.1, size.0, size.1),
            &[scene.tex, ga.tex],
            |gl, p| {
                // SAFETY: uniforms of the composite program, in use.
                unsafe {
                    gl.uniform_1_i32(gl.get_uniform_location(*p, "u_scene").as_ref(), 0);
                    gl.uniform_1_i32(gl.get_uniform_location(*p, "u_bloom").as_ref(), 1);
                    gl.uniform_1_f32(gl.get_uniform_location(*p, "u_strength").as_ref(), strength);
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
