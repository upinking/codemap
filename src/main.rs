mod camera;
mod index;
mod layout;
mod render;
mod screenshot;
mod text;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{mpsc, Arc};
use std::time::Instant;

use camera::Camera;
use index::{Progress, Scene};
use render::{CodeCell, Label, Quad, QuadRenderer};
use render::LineCache;
use text::{TextItem, TextPass};
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowId};

struct FrameState {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    quads_r: QuadRenderer,
    text: TextPass,
    cam: Camera,
    quads: Vec<Quad>,
    labels: Vec<Label>,
    code: Vec<CodeCell>,
    cursor: (f64, f64),
    dragging: bool,
    last_frame: Instant,
    fps: f64,
    fps_accum: f64,
    fps_frames: u32,
    frame_count: u32,
    code_cache: HashMap<PathBuf, Arc<Vec<Box<str>>>>,
    lines_cache: LineCache,
}

struct App {
    repo: PathBuf,
    rx: Option<mpsc::Receiver<Scene>>,
    progress: Arc<Progress>,
    scene: Option<Scene>,
    state: Option<FrameState>,
    index_started: bool,
}

impl App {
    fn start_index(&mut self, el: &ActiveEventLoop) {
        if self.index_started {
            return;
        }
        self.index_started = true;
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        let repo = self.repo.clone();
        let progress = self.progress.clone();
        std::thread::spawn(move || {
            let t = Instant::now();
            let scene = index::build(&repo, &progress);
            println!("indexed in {:.1}s", t.elapsed().as_secs_f64());
            let _ = tx.send(scene);
        });
        let _ = el;
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title(format!("codemap — {}", self.repo.display()))
            .with_inner_size(PhysicalSize::new(1600, 1000));
        let window = Arc::new(el.create_window(attrs).unwrap());

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let surface = instance.create_surface(window.clone()).unwrap();
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            compatible_surface: Some(&surface),
            ..Default::default()
        }))
        .unwrap();
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("device"),
            ..Default::default()
        }))
        .unwrap();

        let caps = surface.get_capabilities(&adapter);
        let format = *caps
            .formats
            .iter()
            .find(|f| f.is_srgb())
            .unwrap_or(&caps.formats[0]);
        eprintln!("surface format: {format:?}");
        let size = window.inner_size();
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            format,
            color_space: wgpu::SurfaceColorSpace::Auto,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::AutoNoVsync,
            desired_maximum_frame_latency: 2,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
        };
        surface.configure(&device, &config);

        let quads_r = QuadRenderer::new(&device, format);
        let mut text = TextPass::new(&device, &queue, format);
        text.resize(&queue, config.width, config.height);

        self.state = Some(FrameState {
            window,
            surface,
            device,
            queue,
            config,
            quads_r,
            text,
            cam: Camera::new(),
            quads: Vec::with_capacity(1 << 14),
            labels: Vec::new(),
            code: Vec::new(),
            cursor: (0.0, 0.0),
            dragging: false,
            last_frame: Instant::now(),
            fps: 0.0,
            fps_accum: 0.0,
            fps_frames: 0,
            frame_count: 0,
            code_cache: HashMap::new(),
            lines_cache: LineCache::new(),
        });
        self.start_index(el);
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(st) = self.state.as_mut() else { return };
        match event {
            WindowEvent::CloseRequested => el.exit(),
            WindowEvent::Resized(size) => {
                if size.width > 0 && size.height > 0 {
                    st.config.width = size.width;
                    st.config.height = size.height;
                    st.surface.configure(&st.device, &st.config);
                    st.text.resize(&st.queue, size.width, size.height);
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                if st.dragging {
                    let (dx, dy) = (position.x - st.cursor.0, position.y - st.cursor.1);
                    st.cam.pan_px(dx, dy);
                }
                st.cursor = (position.x, position.y);
            }
            WindowEvent::MouseInput { state, button: MouseButton::Left, .. } => {
                st.dragging = state == ElementState::Pressed;
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let dy = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y as f64 * 0.0035,
                    MouseScrollDelta::PixelDelta(p) => p.y * 0.0012,
                };
                let factor = (dy * 3.0).exp();
                let (sw, sh) = (st.config.width as f64, st.config.height as f64);
                st.cam.zoom_at(st.cursor.0, st.cursor.1, sw, sh, factor);
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.logical_key == winit::keyboard::Key::Named(winit::keyboard::NamedKey::Escape)
                {
                    el.exit();
                }
            }
            WindowEvent::RedrawRequested => {
                let now = Instant::now();
                let dt = (now - st.last_frame).as_secs_f64().min(0.1);
                st.last_frame = now;

                if let Some(rx) = self.rx.as_ref() {
                    if let Ok(scene) = rx.try_recv() {
                        let r = &scene.nodes[scene.root];
                        st.cam.fit(r.rect[2] as f64, r.rect[3] as f64, st.config.width as f64, st.config.height as f64);
                        self.scene = Some(scene);
                    }
                }

                st.cam.update(dt);
                st.fps_accum += dt;
                st.fps_frames += 1;
                if st.fps_accum >= 0.5 {
                    st.fps = st.fps_frames as f64 / st.fps_accum;
                    st.fps_accum = 0.0;
                    st.fps_frames = 0;
                }

                let (sw, sh) = (st.config.width as f64, st.config.height as f64);
                let view = st.cam.view(sw, sh);

                if let Some(scene) = &self.scene {
                    render::collect(scene, &view, &mut st.quads, &mut st.labels, &mut st.code, &mut st.lines_cache);
                } else {
                    st.quads.clear();
                    st.labels.clear();
                    st.code.clear();
                }

                let mut items: Vec<TextItem> = st.labels.iter().map(label_item).collect();
                for cell in st.code.iter() {
                    let max_len = self
                        .scene
                        .as_ref()
                        .map(|s| st.lines_cache.get(s, cell.node).max_len)
                        .unwrap_or(20);
                    if let Some(item) = code_item(cell, &mut st.code_cache, max_len, sh) {
                        items.push(item);
                    }
                }

                let hud = hud_text(&self.repo, &self.progress, self.scene.as_ref(), st.fps, view.scale);
                items.push(TextItem {
                    text: hud,
                    x: 12.0,
                    y: 10.0,
                    size: 15.0,
                    line_h: 21.0,
                    color: [0.55, 0.95, 0.6, 1.0],
                    clip: [0.0, 0.0, sw as f32, 120.0],
                    mono: true,
                });

                let frame = match st.surface.get_current_texture() {
                    wgpu::CurrentSurfaceTexture::Success(f)
                    | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
                    _ => {
                        st.window.request_redraw();
                        return;
                    }
                };
                let frame_view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
                let mut encoder = st.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") });
                {
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("pass"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &frame_view,
                            resolve_target: None,
                            depth_slice: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.043, g: 0.047, b: 0.06, a: 1.0 }),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        ..Default::default()
                    });
                    st.quads_r.draw(&st.device, &st.queue, &st.quads, st.config.width as f32, st.config.height as f32, &mut pass);
                    st.text.render(&st.device, &st.queue, &items, &mut pass);
                }
                st.queue.submit(Some(encoder.finish()));
                if std::env::var("CODEMAP_LIVE_SHOT").is_ok() {
                    st.frame_count += 1;
                    if st.frame_count == 60 {
                        dump_frame(st, &frame);
                    }
                }
                st.queue.present(frame);
                st.window.request_redraw();
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _el: &ActiveEventLoop) {
        if let Some(st) = &self.state {
            st.window.request_redraw();
        }
    }
}

fn hud_text(repo: &Path, progress: &Progress, scene: Option<&Scene>, fps: f64, scale: f64) -> String {
    let rss = rss_mb();
    match scene {
        None => {
            let phase = progress.phase.load(Ordering::Relaxed);
            let phase_s = match phase {
                0 | 1 => "walking files",
                2 => "counting lines",
                3 => "building layout",
                _ => "finishing",
            };
            let files = progress.files_walked.load(Ordering::Relaxed);
            let counted = progress.files_counted.load(Ordering::Relaxed);
            let total = progress.total_files.load(Ordering::Relaxed).max(1);
            let lines = progress.lines.load(Ordering::Relaxed);
            format!(
                "codemap — {}\nindexing: {} ... {:>7} files | lines {:>12} | counted {}/{} ({:.0}%)\nRSS {:.0} MB",
                repo.display(),
                phase_s,
                files,
                lines,
                counted,
                total,
                counted as f64 / total as f64 * 100.0,
                rss
            )
        }
        Some(scene) => {
            let s = &scene.stats;
            format!(
                "codemap — {} | {:>12} lines | {:>7} files | {:>6} dirs\n{:.0} FPS | zoom {:.3} px/unit | RSS {:.0} MB | Esc quit",
                repo.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                s.lines,
                s.files,
                s.dirs,
                fps,
                scale,
                rss
            )
        }
    }
}

pub fn label_item(l: &Label) -> TextItem {
    TextItem {
        text: l.text.clone(),
        x: l.x,
        y: l.y,
        size: l.size,
        line_h: l.size * 1.2,
        color: if l.bright { [0.85, 0.88, 0.95, 1.0] } else { [1.0, 1.0, 1.0, 0.92] },
        clip: [l.x - 2.0, l.y - 2.0, l.w + 4.0, l.h + 4.0],
        mono: false,
    }
}

pub fn code_item(
    cell: &CodeCell,
    cache: &mut HashMap<PathBuf, Arc<Vec<Box<str>>>>,
    max_len: u32,
    sh: f64,
) -> Option<TextItem> {
    let font = (cell.w / (0.62 * max_len.max(20) as f32)).clamp(4.0, 64.0);
    if font < 5.0 {
        return None;
    }
    let line_h = font * 1.25;
    let lines = cache.entry(cell.path.clone()).or_insert_with(|| load_code(&cell.path)).clone();
    let start = ((0.0f32 - cell.y).max(0.0) / line_h) as usize;
    if start >= lines.len() {
        return None;
    }
    let visible = (sh as f32 / line_h).ceil() as usize + 2;
    let end = (start + visible).min(lines.len());
    let mut text = String::new();
    for i in start..end {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&lines[i]);
    }
    Some(TextItem {
        text,
        x: cell.x.max(0.0) + 6.0,
        y: cell.y + 4.0 + start as f32 * line_h,
        size: font,
        line_h,
        color: [0.83, 0.85, 0.9, 1.0],
        clip: [cell.x, cell.y, cell.w, cell.h],
        mono: true,
    })
}

fn dump_frame(st: &FrameState, frame: &wgpu::SurfaceTexture) {
    let (w, h) = (st.config.width, st.config.height);
    let stride = (w * 4 + 255) / 256 * 256;
    let buffer = st.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("live readback"),
        size: (stride * h) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = st.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("dump") });
    enc.copy_texture_to_buffer(
        frame.texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(stride),
                rows_per_image: Some(h),
            },
        },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
    st.queue.submit(Some(enc.finish()));
    buffer.slice(..).map_async(wgpu::MapMode::Read, |_| {});
    let _ = st.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
    let Ok(data) = buffer.slice(..).get_mapped_range() else { return };
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    for (px, src) in rgba.chunks_mut(4).zip(data.chunks(stride as usize).flat_map(|row| row[..(w * 4) as usize].chunks(4))) {
        px[0] = src[2];
        px[1] = src[1];
        px[2] = src[0];
        px[3] = src[3];
    }
    drop(data);
    buffer.unmap();
    let Ok(file) = std::fs::File::create("/tmp/live_frame.png") else { return };
    let mut enc = png::Encoder::new(file, w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    if let Ok(mut writer) = enc.write_header() {
        let _ = writer.write_image_data(&rgba);
        eprintln!("dumped /tmp/live_frame.png");
    }
}

fn load_code(path: &Path) -> Arc<Vec<Box<str>>> {
    let data = std::fs::read(path).unwrap_or_default();
    let text = String::from_utf8_lossy(&data);
    let mut lines: Vec<Box<str>> = text.lines().take(4000).map(|l| l.replace('\t', "    ").into_boxed_str()).collect();
    if lines.is_empty() {
        lines.push("".into());
    }
    Arc::new(lines)
}

#[repr(C)]
struct Rusage([u64; 32]);

fn rss_mb() -> f64 {
    #[cfg(unix)]
    unsafe {
        extern "C" {
            fn getrusage(who: i32, usage: *mut Rusage) -> i32;
        }
        let mut ru = Rusage([0; 32]);
        if getrusage(0, &mut ru) == 0 {
            return ru.0[4] as f64 / (1024.0 * 1024.0);
        }
    }
    0.0
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut repo = PathBuf::from(".");
    let mut shot: Option<PathBuf> = None;
    let mut at: Option<String> = None;
    let (mut w, mut h) = (1920u32, 1200u32);
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--screenshot" => {
                i += 1;
                shot = Some(PathBuf::from(&args[i]));
            }
            "--at" => {
                i += 1;
                at = Some(args[i].clone());
            }
            "--size" => {
                i += 1;
                let (a, b) = args[i].split_once('x').unwrap_or(("1920", "1200"));
                w = a.parse().unwrap_or(1920);
                h = b.parse().unwrap_or(1200);
            }
            p => repo = PathBuf::from(p),
        }
        i += 1;
    }
    if !repo.exists() {
        eprintln!("repo not found: {}", repo.display());
        std::process::exit(1);
    }

    if let Some(out) = shot {
        screenshot::capture(&repo, &out, at.as_deref(), w, h);
        return;
    }
    println!("indexing {} ...", repo.display());

    let event_loop = EventLoop::new().unwrap();
    let mut app = App {
        repo,
        rx: None,
        progress: Arc::new(Progress::default()),
        scene: None,
        state: None,
        index_started: false,
    };
    event_loop.run_app(&mut app).unwrap();
}
