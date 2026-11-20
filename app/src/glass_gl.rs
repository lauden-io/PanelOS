//! Rain on the glass, drawn by the GPU over the frame Skia has just finished
//! (feature `gpu`): the frame is copied at half size into a texture, then each
//! drop, bead and wet streak of `glass.rs` is one instanced quad. A drop's
//! fragment shader looks through it at that copy (an inverted, wider view with
//! a dark rim, a caustic and a highlight); a streak only darkens what is under
//! it. One small copy and a few hundred tiny quads, nothing when the glass is
//! dry. Skia keeps its own idea of the GL state, so everything touched here is
//! saved first and put back afterwards.

use std::cell::RefCell;
use std::num::NonZeroU32;

use glow::HasContext;

use crate::glass::INSTANCE_FLOATS;

const VS: &str = r#"#version 300 es
layout(location = 0) in vec2 corner;
layout(location = 1) in vec4 geo;
layout(location = 2) in vec2 par;
uniform vec2 size;
flat out vec4 g;
flat out vec2 q;
void main() {
    vec2 lo;
    vec2 hi;
    if (par.y < 0.5) {
        // a streak point: one pixel row
        lo = vec2(geo.x - geo.z - 1.0, geo.y);
        hi = vec2(geo.x + geo.z + 1.0, geo.y + 1.0);
    } else {
        lo = geo.xy - geo.zw - 1.0;
        hi = geo.xy + geo.zw + 1.0;
    }
    vec2 p = mix(floor(lo), ceil(hi), corner);
    g = geo;
    q = par;
    gl_Position = vec4(p.x / size.x * 2.0 - 1.0, 1.0 - p.y / size.y * 2.0, 0.0, 1.0);
}
"#;

const FS: &str = r#"#version 300 es
precision highp float;
uniform sampler2D src;
uniform vec2 size;
uniform float fade;
flat in vec4 g;
flat in vec2 q;
out vec4 o;
// the frame under the glass at a screen point (y down)
vec3 tap(vec2 s) {
    return texture(src, vec2(s.x / size.x, 1.0 - s.y / size.y)).rgb;
}
float sq(float x) { return x * x; }
void main() {
    vec2 px = vec2(gl_FragCoord.x, size.y - gl_FragCoord.y);
    if (q.y < 0.5) {
        // wet streak: a little darker, its left edge clear
        float dx = (px.x - g.x) / g.z;
        if (dx < -0.5 || dx > 1.0) discard;
        o = vec4(0.0, 0.0, 0.0, 0.2 * q.x * (1.0 - dx * dx) * fade);
        return;
    }
    bool big = q.y > 1.5;
    vec2 n = (px - g.xy) / g.zw;
    float rho2 = dot(n, n);
    if (rho2 > 1.08) discard;
    float rho = sqrt(rho2);
    float a_edge = clamp((1.0 - rho) * min(g.z, g.w) + 0.6, 0.0, 1.0);
    // inverted, wider view (fish-eye towards the rim); small beads barely refract
    float k = (big ? 3.2 : 1.6) * (0.55 + 0.45 * rho2);
    vec3 c = tap(g.xy - n * g.zw * k) * 255.0;
    // water: a little brighter and softer
    c = c * 0.90 + vec3(14.0, 14.0, 16.0);
    // dark rim, stronger at the top
    float rim = clamp((rho - 0.72) / 0.28, 0.0, 1.0);
    float top = clamp(-n.y, 0.0, 1.0);
    c *= 1.0 - (0.55 * rim * rim) * (0.55 + 0.45 * top);
    if (big) {
        // bright caustic crescent near the bottom, highlight at the upper left
        float cc = exp(-sq((rho - 0.80) / 0.11)) * clamp(n.y - 0.25, 0.0, 1.0) * 1.7;
        c += vec3(95.0, 95.0, 100.0) * cc;
        float s = exp(-(sq((n.x + 0.36) / 0.20) + sq((n.y + 0.44) / 0.15)));
        c += vec3(210.0, 210.0, 215.0) * s;
    } else {
        float s = exp(-(sq((n.x + 0.3) / 0.35) + sq((n.y + 0.35) / 0.35)));
        c += vec3(90.0, 90.0, 95.0) * s;
    }
    o = vec4(clamp(c / 255.0, 0.0, 1.0), a_edge * q.x * fade);
}
"#;

struct Res {
    prog: glow::Program,
    vao: glow::VertexArray,
    inst: glow::Buffer,
    tex: glow::Texture,
    fbo: glow::Framebuffer,
    /// Size of the frame copy (half the screen; the whole screen when the window is multisampled).
    copy: (i32, i32),
    u_size: Option<glow::UniformLocation>,
    u_src: Option<glow::UniformLocation>,
    u_fade: Option<glow::UniformLocation>,
    checks: u8,
}

struct Saved {
    active: i32,
    tex: i32,
    sampler: i32,
    prog: i32,
    array_buf: i32,
    vao: i32,
    draw_fb: i32,
    read_fb: i32,
    viewport: [i32; 4],
    blend: bool,
    scissor: bool,
    stencil: bool,
    depth: bool,
    cull: bool,
    blend_src_rgb: i32,
    blend_dst_rgb: i32,
    blend_src_a: i32,
    blend_dst_a: i32,
    blend_eq_rgb: i32,
    blend_eq_a: i32,
    mask: [i32; 4],
}

enum State {
    New,
    Ready(Res),
    Failed,
}

thread_local! {
    static STATE: RefCell<State> = const { RefCell::new(State::New) };
}

fn nz(v: i32) -> Option<NonZeroU32> {
    NonZeroU32::new(v as u32)
}

fn bytes(v: &[f32]) -> &[u8] {
    // SAFETY: f32 has no padding and any byte pattern is a valid u8.
    unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, std::mem::size_of_val(v)) }
}

/// # Safety
/// The GL context of `gl` must be current.
unsafe fn setup(gl: &glow::Context, w: i32, h: i32) -> Result<Res, String> {
    let compile = |kind: u32, src: &str| -> Result<glow::Shader, String> {
        let s = gl.create_shader(kind)?;
        gl.shader_source(s, src);
        gl.compile_shader(s);
        if !gl.get_shader_compile_status(s) {
            return Err(gl.get_shader_info_log(s));
        }
        Ok(s)
    };
    let vs = compile(glow::VERTEX_SHADER, VS)?;
    let fs = compile(glow::FRAGMENT_SHADER, FS)?;
    let prog = gl.create_program()?;
    gl.attach_shader(prog, vs);
    gl.attach_shader(prog, fs);
    gl.link_program(prog);
    if !gl.get_program_link_status(prog) {
        return Err(gl.get_program_info_log(prog));
    }
    gl.detach_shader(prog, vs);
    gl.detach_shader(prog, fs);
    gl.delete_shader(vs);
    gl.delete_shader(fs);
    let vao = gl.create_vertex_array()?;
    let quad = gl.create_buffer()?;
    let inst = gl.create_buffer()?;
    gl.bind_vertex_array(Some(vao));
    gl.bind_buffer(glow::ARRAY_BUFFER, Some(quad));
    let corners: [f32; 8] = [0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 1.0];
    gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes(&corners), glow::STATIC_DRAW);
    gl.enable_vertex_attrib_array(0);
    gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, 8, 0);
    gl.bind_buffer(glow::ARRAY_BUFFER, Some(inst));
    let stride = (INSTANCE_FLOATS * 4) as i32;
    gl.enable_vertex_attrib_array(1);
    gl.vertex_attrib_pointer_f32(1, 4, glow::FLOAT, false, stride, 0);
    gl.vertex_attrib_divisor(1, 1);
    gl.enable_vertex_attrib_array(2);
    gl.vertex_attrib_pointer_f32(2, 2, glow::FLOAT, false, stride, 16);
    gl.vertex_attrib_divisor(2, 1);
    gl.bind_vertex_array(None);
    // The frame copy: half size is plenty for what a drop shows (a small, blurred, inverted view) and the
    // scaled copy filters it; a multisampled window can only be copied at its own size.
    gl.bind_framebuffer(glow::FRAMEBUFFER, None);
    let samples = gl.get_parameter_i32(glow::SAMPLE_BUFFERS);
    let copy = if samples > 0 { (w, h) } else { ((w + 1) / 2, (h + 1) / 2) };
    let tex = gl.create_texture()?;
    gl.bind_texture(glow::TEXTURE_2D, Some(tex));
    gl.tex_storage_2d(glow::TEXTURE_2D, 1, glow::RGBA8, copy.0, copy.1);
    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR as i32);
    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
    let fbo = gl.create_framebuffer()?;
    gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
    gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(tex), 0);
    let status = gl.check_framebuffer_status(glow::FRAMEBUFFER);
    gl.bind_framebuffer(glow::FRAMEBUFFER, None);
    if status != glow::FRAMEBUFFER_COMPLETE {
        return Err(format!("frame copy target incomplete (0x{status:x})"));
    }
    let u_size = gl.get_uniform_location(prog, "size");
    let u_src = gl.get_uniform_location(prog, "src");
    let u_fade = gl.get_uniform_location(prog, "fade");
    eprintln!("nspanel-app: glass pass ready, frame copy {}x{}", copy.0, copy.1);
    Ok(Res { prog, vao, inst, tex, fbo, copy, u_size, u_src, u_fade, checks: 0 })
}

/// # Safety
/// The GL context of `gl` must be current.
unsafe fn save(gl: &glow::Context) -> Saved {
    let active = gl.get_parameter_i32(glow::ACTIVE_TEXTURE);
    gl.active_texture(glow::TEXTURE0);
    let mut viewport = [0; 4];
    gl.get_parameter_i32_slice(glow::VIEWPORT, &mut viewport);
    let mut mask = [0; 4];
    gl.get_parameter_i32_slice(glow::COLOR_WRITEMASK, &mut mask);
    Saved {
        active,
        tex: gl.get_parameter_i32(glow::TEXTURE_BINDING_2D),
        sampler: gl.get_parameter_i32(glow::SAMPLER_BINDING),
        prog: gl.get_parameter_i32(glow::CURRENT_PROGRAM),
        array_buf: gl.get_parameter_i32(glow::ARRAY_BUFFER_BINDING),
        vao: gl.get_parameter_i32(glow::VERTEX_ARRAY_BINDING),
        draw_fb: gl.get_parameter_i32(glow::DRAW_FRAMEBUFFER_BINDING),
        read_fb: gl.get_parameter_i32(glow::READ_FRAMEBUFFER_BINDING),
        viewport,
        blend: gl.is_enabled(glow::BLEND),
        scissor: gl.is_enabled(glow::SCISSOR_TEST),
        stencil: gl.is_enabled(glow::STENCIL_TEST),
        depth: gl.is_enabled(glow::DEPTH_TEST),
        cull: gl.is_enabled(glow::CULL_FACE),
        blend_src_rgb: gl.get_parameter_i32(glow::BLEND_SRC_RGB),
        blend_dst_rgb: gl.get_parameter_i32(glow::BLEND_DST_RGB),
        blend_src_a: gl.get_parameter_i32(glow::BLEND_SRC_ALPHA),
        blend_dst_a: gl.get_parameter_i32(glow::BLEND_DST_ALPHA),
        blend_eq_rgb: gl.get_parameter_i32(glow::BLEND_EQUATION_RGB),
        blend_eq_a: gl.get_parameter_i32(glow::BLEND_EQUATION_ALPHA),
        mask,
    }
}

/// # Safety
/// The GL context of `gl` must be current.
unsafe fn restore(gl: &glow::Context, s: &Saved) {
    let set = |cap: u32, on: bool| if on { gl.enable(cap) } else { gl.disable(cap) };
    set(glow::BLEND, s.blend);
    set(glow::SCISSOR_TEST, s.scissor);
    set(glow::STENCIL_TEST, s.stencil);
    set(glow::DEPTH_TEST, s.depth);
    set(glow::CULL_FACE, s.cull);
    gl.blend_func_separate(s.blend_src_rgb as u32, s.blend_dst_rgb as u32, s.blend_src_a as u32, s.blend_dst_a as u32);
    gl.blend_equation_separate(s.blend_eq_rgb as u32, s.blend_eq_a as u32);
    gl.color_mask(s.mask[0] != 0, s.mask[1] != 0, s.mask[2] != 0, s.mask[3] != 0);
    gl.viewport(s.viewport[0], s.viewport[1], s.viewport[2], s.viewport[3]);
    gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, nz(s.draw_fb).map(glow::NativeFramebuffer));
    gl.bind_framebuffer(glow::READ_FRAMEBUFFER, nz(s.read_fb).map(glow::NativeFramebuffer));
    gl.bind_vertex_array(nz(s.vao).map(glow::NativeVertexArray));
    gl.bind_buffer(glow::ARRAY_BUFFER, nz(s.array_buf).map(glow::NativeBuffer));
    gl.use_program(nz(s.prog).map(glow::NativeProgram));
    gl.bind_sampler(0, nz(s.sampler).map(glow::NativeSampler));
    gl.bind_texture(glow::TEXTURE_2D, nz(s.tex).map(glow::NativeTexture));
    gl.active_texture(s.active as u32);
}

/// Draws the glass's drops over the window's frame (`w` × `h` pixels, the default framebuffer), after Skia has
/// flushed it and before the swap. Does nothing while the glass is dry or off screen.
pub fn draw(gl: &glow::Context, w: i32, h: i32) {
    let started = std::time::Instant::now();
    match crate::glass::with_instances(|inst, fade| draw_instances(gl, w, h, inst, fade)) {
        None => {}
        Some(Ok(())) => {
            if crate::gpu::FRAME_STATS.load(std::sync::atomic::Ordering::Relaxed) {
                crate::glass::note_pass(started.elapsed().as_secs_f32() * 1000.0);
                crate::glass::report_if_due();
            }
        }
        Some(Err(e)) => {
            // the frames stay right without it
            eprintln!("nspanel-app: glass pass off: {e}");
            crate::glass::set_pass(false);
        }
    }
}

fn draw_instances(gl: &glow::Context, w: i32, h: i32, inst: &[f32], fade: f32) -> Result<(), String> {
    STATE.with(|st| {
        let mut st = st.borrow_mut();
        if matches!(*st, State::Failed) {
            return Ok(());
        }
        // SAFETY: called inside the renderer's pre-present callback, with its GL context current.
        unsafe {
            let saved = save(gl);
            let checking = !matches!(&*st, State::Ready(r) if r.checks >= 3);
            if checking {
                // errors left from before are not this pass's
                for _ in 0..8 {
                    if gl.get_error() == glow::NO_ERROR {
                        break;
                    }
                }
            }
            if matches!(*st, State::New) {
                match setup(gl, w, h) {
                    Ok(r) => *st = State::Ready(r),
                    Err(e) => {
                        restore(gl, &saved);
                        *st = State::Failed;
                        return Err(format!("GL setup failed: {e}"));
                    }
                }
            }
            let State::Ready(r) = &mut *st else { return Ok(()) };
            gl.disable(glow::SCISSOR_TEST);
            gl.disable(glow::STENCIL_TEST);
            gl.disable(glow::DEPTH_TEST);
            gl.disable(glow::CULL_FACE);
            gl.color_mask(true, true, true, true);
            // the frame as it is, into the copy
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, None);
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(r.fbo));
            let filter = if r.copy == (w, h) { glow::NEAREST } else { glow::LINEAR };
            gl.blit_framebuffer(0, 0, w, h, 0, 0, r.copy.0, r.copy.1, glow::COLOR_BUFFER_BIT, filter);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            // the drops over it
            gl.enable(glow::BLEND);
            gl.blend_func_separate(glow::SRC_ALPHA, glow::ONE_MINUS_SRC_ALPHA, glow::ZERO, glow::ONE);
            gl.blend_equation_separate(glow::FUNC_ADD, glow::FUNC_ADD);
            gl.viewport(0, 0, w, h);
            gl.use_program(Some(r.prog));
            gl.uniform_2_f32(r.u_size.as_ref(), w as f32, h as f32);
            gl.uniform_1_i32(r.u_src.as_ref(), 0);
            gl.uniform_1_f32(r.u_fade.as_ref(), fade);
            gl.bind_sampler(0, None);
            gl.bind_texture(glow::TEXTURE_2D, Some(r.tex));
            gl.bind_vertex_array(Some(r.vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(r.inst));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes(inst), glow::STREAM_DRAW);
            gl.draw_arrays_instanced(glow::TRIANGLE_STRIP, 0, 4, (inst.len() / INSTANCE_FLOATS) as i32);
            // the first frames: a GL error turns the pass off for good
            if r.checks < 3 {
                r.checks += 1;
                let e = gl.get_error();
                if e != glow::NO_ERROR {
                    restore(gl, &saved);
                    *st = State::Failed;
                    return Err(format!("GL error 0x{e:x}"));
                }
            }
            restore(gl, &saved);
        }
        Ok(())
    })
}
