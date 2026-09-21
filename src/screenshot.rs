use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use crate::camera::Camera;
use crate::index::{self, node_path, Progress};
use crate::render::{self, CodeCell, QuadRenderer};
use crate::text::{TextItem, TextPass};
use crate::{code_item, label_item};

pub fn capture(repo: &Path, out: &Path, at: Option<&str>, width: u32, height: u32) {
    let progress = Arc::new(Progress::default());
    let t = Instant::now();
    let scene = index::build(repo, &progress);
    println!(
        "indexed in {:.1}s: {} files, {} dirs, {} lines, {:.2} GB",
        t.elapsed().as_secs_f64(),
        scene.stats.files,
        scene.stats.dirs,
        scene.stats.lines,
        scene.stats.bytes as f64 / 1e9
    );

    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).unwrap();
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("offscreen"),
        ..Default::default()
    }))
    .unwrap();

    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("target"),
        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let tex_view = texture.create_view(&wgpu::TextureViewDescriptor::default());

    if std::env::var("CODEMAP_DUMP").is_ok() {
        for (i, n) in scene.nodes.iter().enumerate().take(40) {
            println!(
                "{:>3} {} w={:>10.1} rect=[{:>9.2},{:>9.2},{:>9.2},{:>9.2}] {}",
                i,
                if n.is_dir { "D" } else { "f" },
                n.weight,
                n.rect[0],
                n.rect[1],
                n.rect[2],
                n.rect[3],
                index::node_name(&scene, n)
            );
        }
    }

    let mut cam = Camera::new();
    let r = scene.nodes[scene.root].rect;
    if let Some(pat) = at {
        let hit = (0..scene.nodes.len()).find(|&i| !scene.nodes[i].is_dir && node_path(&scene, &scene.nodes[i]).contains(pat));
        match hit {
            Some(i) => {
                let nr = scene.nodes[i].rect;
                cam.fit(nr[2] as f64, nr[3] as f64, width as f64, height as f64);
                cam.cx = nr[0] as f64 + nr[2] as f64 / 2.0;
                cam.cy = nr[1] as f64 + nr[3] as f64 / 2.0;
                cam.tx = cam.cx;
                cam.ty = cam.cy;
                println!("focused on {}", node_path(&scene, &scene.nodes[i]));
            }
            None => eprintln!("no file matching '{}'", pat),
        }
    } else {
        cam.fit(r[2] as f64, r[3] as f64, width as f64, height as f64);
    }
    cam.update(10.0);
    if let Ok(z) = std::env::var("CODEMAP_ZOOM") {
        if let Ok(f) = z.parse::<f64>() {
            cam.scale *= f;
            cam.tscale = cam.scale;
        }
    }
    let view = cam.view(width as f64, height as f64);

    let mut quads = Vec::new();
    let mut labels = Vec::new();
    let mut code = Vec::new();
    let mut lines_cache = render::LineCache::new();
    render::collect(&scene, &view, &mut quads, &mut labels, &mut code, &mut lines_cache);
    println!("quads: {}, labels: {}, code cells: {}", quads.len(), labels.len(), code.len());

    let mut items: Vec<TextItem> = labels.iter().map(label_item).collect();
    let mut cache: HashMap<PathBuf, Arc<Vec<Box<str>>>> = HashMap::new();
    for cell in code.iter() {
        let max_len = lines_cache.get(&scene, cell.node).max_len;
        if let Some(item) = code_item(cell, &mut cache, max_len, height as f64) {
            items.push(item);
        }
    }
    items.push(TextItem {
        text: crate::hud_text(repo, &progress, Some(&scene), 0.0, view.scale),
        x: 12.0,
        y: 10.0,
        size: 15.0,
        line_h: 21.0,
        color: [0.55, 0.95, 0.6, 1.0],
        clip: [0.0, 0.0, width as f32, 140.0],
        mono: true,
    });

    let mut quads_r = QuadRenderer::new(&device, format);
    let mut text = TextPass::new(&device, &queue, format);
    text.resize(&queue, width, height);
    if std::env::var("CODEMAP_DUMP").is_ok() {
        for (k, it) in items.iter().enumerate() {
            eprintln!(
                "item {k} pos=({:.0},{:.0}) size={:.1} line_h={:.1} clip=[{:.0},{:.0},{:.0},{:.0}] mono={} text={:.40}",
                it.x, it.y, it.size, it.line_h, it.clip[0], it.clip[1], it.clip[2], it.clip[3], it.mono, it.text
            );
        }
    }

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("shot") });
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &tex_view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.043, g: 0.047, b: 0.06, a: 1.0 }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        quads_r.draw(&device, &queue, &quads, width as f32, height as f32, &mut pass);
        text.render(&device, &queue, &items, &mut pass);
    }

    let stride = (width * 4 + 255) / 256 * 256;
    let buf_size = (stride * height) as u64;
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: buf_size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(stride),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
    );
    queue.submit(Some(encoder.finish()));
    readback.slice(..).map_async(wgpu::MapMode::Read, |_| {});
    let _ = device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
    let data = readback.slice(..).get_mapped_range().unwrap();

    let mut rgba = vec![0u8; (width * height * 4) as usize];
    for (row_out, row_in) in rgba
        .chunks_mut((width * 4) as usize)
        .zip(data.chunks(stride as usize))
    {
        row_out.copy_from_slice(&row_in[..(width * 4) as usize]);
    }
    drop(data);
    readback.unmap();

    let file = std::fs::File::create(out).unwrap();
    let mut enc = png::Encoder::new(file, width, height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().unwrap();
    writer.write_image_data(&rgba).unwrap();
    drop(writer);
    println!("wrote {}", out.display());
}
