use crate::index::{node_name, Lang, Scene};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use wgpu::util::DeviceExt;

#[derive(Clone, Copy)]
pub struct View {
    pub cx: f64,
    pub cy: f64,
    pub scale: f64,
    pub sw: f64,
    pub sh: f64,
}

impl View {
    pub fn sx(&self, wx: f64) -> f64 {
        (wx - self.cx) * self.scale + self.sw / 2.0
    }
    pub fn sy(&self, wy: f64) -> f64 {
        (wy - self.cy) * self.scale + self.sh / 2.0
    }
    pub fn wx(&self, sx: f64) -> f64 {
        (sx - self.sw / 2.0) / self.scale + self.cx
    }
    pub fn wy(&self, sy: f64) -> f64 {
        (sy - self.sh / 2.0) / self.scale + self.cy
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Quad {
    pub rect: [f32; 4],
    pub color: [f32; 4],
    pub edge: [f32; 4],
}

pub struct Label {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub size: f32,
    pub bright: bool,
}

pub struct CodeCell {
    pub path: PathBuf,
    pub node: usize,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

pub struct LineMetrics {
    pub max_len: u32,
    pub lines: Vec<u32>,
}

pub struct LineCache {
    map: HashMap<usize, Arc<LineMetrics>>,
    order: Vec<usize>,
}

impl LineCache {
    pub fn new() -> Self {
        LineCache { map: HashMap::new(), order: Vec::new() }
    }

    pub fn get(&mut self, scene: &Scene, node: usize) -> Arc<LineMetrics> {
        if let Some(m) = self.map.get(&node) {
            return m.clone();
        }
        let path = crate::index::node_path(scene, &scene.nodes[node]);
        let lang = scene.nodes[node].lang;
        let m = Arc::new(build_metrics(path, lang));
        self.map.insert(node, m.clone());
        self.order.push(node);
        if self.order.len() > 96 {
            let old = self.order.remove(0);
            self.map.remove(&old);
        }
        m
    }
}

fn line_class(line: &[u8], lang: Lang) -> u32 {
    let t = match line.iter().position(|&c| c != b' ' && c != b'\t') {
        Some(p) => &line[p..],
        None => return 0,
    };
    if t.is_empty() {
        return 0;
    }
    if t.starts_with(b"//") || t.starts_with(b"/*") || t[0] == b'*' {
        return 1;
    }
    if t[0] == b'#' {
        return match lang {
            Lang::Python | Lang::Build | Lang::Markdown => 1,
            _ => 2,
        };
    }
    3
}

fn build_metrics(path: &str, lang: Lang) -> LineMetrics {
    let mut metrics = LineMetrics { max_len: 1, lines: Vec::new() };
    let Ok(data) = std::fs::read(path) else { return metrics };
    let data = if data.len() > 12 * 1024 * 1024 { &data[..12 * 1024 * 1024] } else { &data[..] };
    let mut raw: Vec<u32> = Vec::new();
    for line in data.split(|&b| b == b'\n') {
        let mut indent = 0usize;
        for &c in line {
            if c == b' ' {
                indent += 1;
            } else if c == b'\t' {
                indent += 4;
            } else {
                break;
            }
        }
        let cls = line_class(line, lang);
        let len = line.len().min(65535) as u32;
        let ind = indent.min(4095) as u32;
        raw.push((cls << 28) | (ind << 16) | len);
    }
    if raw.len() > 50_000 {
        let stride = raw.len() / 50_000;
        raw = raw.into_iter().step_by(stride).collect();
    }
    for &e in &raw {
        metrics.max_len = metrics.max_len.max(e & 0xffff);
    }
    metrics.lines = raw;
    metrics
}

const DIR_SUBDIV_W: f64 = 26.0;
const DIR_SUBDIV_H: f64 = 18.0;

pub fn collect(
    scene: &Scene,
    view: &View,
    quads: &mut Vec<Quad>,
    labels: &mut Vec<Label>,
    code: &mut Vec<CodeCell>,
    cache: &mut LineCache,
) {
    quads.clear();
    labels.clear();
    code.clear();
    let mut budget: usize = 150_000;
    walk(scene, view, scene.root, quads, labels, code, cache, &mut budget);
}

fn walk(
    scene: &Scene,
    view: &View,
    i: usize,
    quads: &mut Vec<Quad>,
    labels: &mut Vec<Label>,
    code: &mut Vec<CodeCell>,
    cache: &mut LineCache,
    budget: &mut usize,
) {
    let n = &scene.nodes[i];
    let r = n.rect;
    let x1 = view.sx(r[0] as f64);
    let y1 = view.sy(r[1] as f64);
    let x2 = view.sx((r[0] + r[2]) as f64);
    let y2 = view.sy((r[1] + r[3]) as f64);

    if x2 < -8.0 || y2 < -8.0 || x1 > view.sw + 8.0 || y1 > view.sh + 8.0 {
        return;
    }
    let wpx = x2 - x1;
    let hpx = y2 - y1;
    if wpx < 0.4 && hpx < 0.4 {
        return;
    }

    if n.is_dir && n.children_count > 0 && wpx > DIR_SUBDIV_W && hpx > DIR_SUBDIV_H {
        let (fill, edge) = dir_colors(n.depth);
        quads.push(Quad {
            rect: [x1 as f32, y1 as f32, wpx as f32, hpx as f32],
            color: [fill[0], fill[1], fill[2], 1.0],
            edge: [edge[0], edge[1], edge[2], if wpx > 3.0 { 1.2 } else { 0.0 }],
        });

        let band = hpx * 0.055;
        if band > 9.0 && wpx > 42.0 && labels.len() < 600 {
            let size = ((band * 0.72).min(24.0).max(9.0)) as f32;
            let text = node_name(scene, n).to_string();
            let chip_w = text.len() as f32 * size * 0.62 + 10.0;
            let band_top = y1.clamp(0.0, (view.sh - band).max(0.0));
            let lx = x1.clamp(0.0, (view.sw - 40.0).max(0.0));
            quads.push(Quad {
                rect: [lx as f32 + 2.0, band_top as f32 + 1.0, chip_w.min(wpx as f32 - 4.0), band as f32 - 2.0],
                color: [0.04, 0.05, 0.06, 0.82],
                edge: [0.0, 0.0, 0.0, 0.0],
            });
            labels.push(Label {
                text,
                x: lx as f32 + 5.0,
                y: band_top as f32 + (band * 0.5 - size as f64 * 0.42) as f32,
                w: wpx as f32 - 10.0,
                h: band as f32,
                size,
                bright: true,
            });
        }

        let start = n.children_start as usize;
        let end = start + n.children_count as usize;
        for &c in &scene.children_ids[start..end] {
            walk(scene, view, c as usize, quads, labels, code, cache, budget);
        }
    } else {
        let (fill, edge) = if n.is_dir {
            dir_colors(n.depth)
        } else if n.lang.is_binary() {
            ([0.26, 0.26, 0.28], [0.16, 0.16, 0.18])
        } else {
            ([0.110, 0.125, 0.150], [0.24, 0.27, 0.31])
        };
        quads.push(Quad {
            rect: [x1 as f32, y1 as f32, wpx.max(0.6) as f32, hpx.max(0.6) as f32],
            color: [fill[0], fill[1], fill[2], 1.0],
            edge: [edge[0], edge[1], edge[2], if wpx > 5.0 && hpx > 5.0 { 1.0 } else { 0.0 }],
        });

        if !n.is_dir && wpx > 70.0 && hpx > 16.0 && labels.len() < 600 {
            let size = ((hpx * 0.55).min(18.0).max(9.0)) as f32;
            let text = node_name(scene, n).to_string();
            let chip_w = text.len() as f32 * size * 0.62 + 10.0;
            let lx = (x1 + 4.0).clamp(4.0, (view.sw - 60.0).max(4.0));
            let ly = (y1 + hpx * 0.5 - size as f64 * 0.55).clamp(4.0, (view.sh - size as f64 * 1.4).max(4.0));
            quads.push(Quad {
                rect: [lx as f32 - 3.0, ly as f32 - size * 0.25, chip_w.min(wpx as f32 - 2.0), size * 1.5],
                color: [0.04, 0.05, 0.06, 0.82],
                edge: [0.0, 0.0, 0.0, 0.0],
            });
            labels.push(Label {
                text,
                x: lx as f32,
                y: ly as f32,
                w: wpx as f32 - 8.0,
                h: hpx as f32,
                size,
                bright: false,
            });
        }

        if !n.is_dir && !n.lang.is_binary() && hpx >= 8.0 && wpx >= 10.0 {
            let m = cache.get(scene, i);
            let font_est = wpx / (0.62 * m.max_len.max(20) as f64);
            if font_est >= 5.0 && wpx > 120.0 && hpx > 60.0 && code.len() < 24 {
                code.push(CodeCell {
                    path: PathBuf::from(crate::index::node_path(scene, n)),
                    node: i,
                    x: x1 as f32,
                    y: y1 as f32,
                    w: wpx as f32,
                    h: hpx as f32,
                });
            } else if font_est < 5.0 && *budget > 0 && !m.lines.is_empty() {
                let strokes = (hpx as usize).min(m.lines.len()).min(*budget);
                let stride = (m.lines.len() / strokes).max(1);
                let pitch = hpx / strokes as f64;
                let cw = wpx / m.max_len.max(1) as f64;
                let sh = (pitch * 0.45).clamp(0.6, 2.0);
                for k in 0..strokes {
                    let e = m.lines[k * stride];
                    let cls = e >> 28;
                    let sc = match cls {
                        1 => [0.35, 0.52, 0.35],
                        2 => [0.42, 0.52, 0.70],
                        3 => [0.55, 0.57, 0.60],
                        _ => continue,
                    };
                    let ind = ((e >> 16) & 0xfff) as f64;
                    let len = (e & 0xffff) as f64;
                    let ix = (ind * cw).min(wpx * 0.5);
                    let lw = (len * cw - ix).max(1.0).min(wpx - ix);
                    quads.push(Quad {
                        rect: [(x1 + ix) as f32, (y1 + k as f64 * pitch) as f32, lw as f32, sh as f32],
                        color: [sc[0], sc[1], sc[2], 1.0],
                        edge: [0.0, 0.0, 0.0, 0.0],
                    });
                }
                *budget = budget.saturating_sub(strokes);
            }
        }
    }
}

fn dir_colors(depth: u16) -> ([f32; 3], [f32; 3]) {
    let t = (depth as f32 * 0.12).min(1.0);
    let l = 0.085 + t * 0.03;
    let fill = crate::index::hsl(215.0, 0.20, l);
    let edge = crate::index::hsl(190.0, 0.65, 0.45);
    (fill, edge)
}

pub struct QuadRenderer {
    pipeline: wgpu::RenderPipeline,
    bgl: wgpu::BindGroupLayout,
    bind_group: wgpu::BindGroup,
    uniform_buf: wgpu::Buffer,
    index_buf: wgpu::Buffer,
    instance_buf: wgpu::Buffer,
    instance_cap: usize,
}

const SHADER: &str = r#"
struct Globals { screen: vec2<f32>, pad: vec2<f32> };
@group(0) @binding(0) var<uniform> g: Globals;

struct Inst { rect: vec4<f32>, color: vec4<f32>, edge: vec4<f32> };
@group(0) @binding(1) var<storage, read> insts: array<Inst>;

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) edge: vec4<f32>,
    @location(3) rect: vec4<f32>,
};

@vertex
fn vs(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> VOut {
    let inst = insts[ii];
    let cx = f32(vi & 1u);
    let cy = f32((vi >> 1u) & 1u);
    let corner = vec2<f32>(cx, cy);
    let px = inst.rect.xy + corner * inst.rect.zw;
    let clip = vec2<f32>(px.x / g.screen.x * 2.0 - 1.0, 1.0 - px.y / g.screen.y * 2.0);
    var out: VOut;
    out.pos = vec4<f32>(clip, 0.0, 1.0);
    out.uv = corner;
    out.color = inst.color;
    out.edge = inst.edge;
    out.rect = inst.rect;
    return out;
}

@fragment
fn fs(in: VOut) -> @location(0) vec4<f32> {
    let dpx = vec2<f32>(
        min(in.uv.x, 1.0 - in.uv.x) * in.rect.z,
        min(in.uv.y, 1.0 - in.uv.y) * in.rect.w,
    );
    let d = min(dpx.x, dpx.y);
    let ew = in.edge.a;
    let t = smoothstep(ew - 1.0, ew + 0.5, d);
    let c = mix(in.edge.rgb, in.color.rgb, t);
    return vec4<f32>(c, in.color.a);
}
"#;

impl QuadRenderer {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("quad shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });

        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("quad uniforms"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let instance_cap = 1 << 16;
        let instance_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("quad instances"),
            size: (instance_cap * std::mem::size_of::<Quad>()) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let index_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("quad indices"),
            contents: bytemuck::cast_slice(&[0u32, 1, 2, 2, 1, 3]),
            usage: wgpu::BufferUsages::INDEX,
        });

        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("quad bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("quad pipeline layout"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("quad bg"),
            layout: &bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: instance_buf.as_entire_binding(),
                },
            ],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("quad pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        QuadRenderer { pipeline, bgl, bind_group, uniform_buf, index_buf, instance_buf, instance_cap }
    }

    pub fn draw(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, quads: &[Quad], sw: f32, sh: f32, pass: &mut wgpu::RenderPass) {
        if quads.is_empty() {
            return;
        }
        if quads.len() > self.instance_cap {
            self.instance_cap = quads.len().next_power_of_two();
            self.instance_buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("quad instances"),
                size: (self.instance_cap * std::mem::size_of::<Quad>()) as u64,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("quad bg"),
                layout: &self.bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: self.uniform_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: self.instance_buf.as_entire_binding(),
                    },
                ],
            });
        }
        queue.write_buffer(&self.uniform_buf, 0, bytemuck::cast_slice(&[sw, sh, 0.0f32, 0.0]));
        queue.write_buffer(&self.instance_buf, 0, bytemuck::cast_slice(quads));

        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_index_buffer(self.index_buf.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..6, 0, 0..quads.len() as u32);
    }
}
