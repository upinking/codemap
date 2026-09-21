use glyphon::{
    Attrs, Buffer, Cache, Color, Family, FontSystem, Metrics, Resolution, Shaping, SwashCache,
    TextArea, TextAtlas, TextBounds, TextRenderer, Viewport,
};

pub struct TextItem {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub size: f32,
    pub line_h: f32,
    pub color: [f32; 4],
    pub clip: [f32; 4],
    pub mono: bool,
}

pub struct TextPass {
    font_system: FontSystem,
    swash: SwashCache,
    cache: Cache,
    atlas: TextAtlas,
    renderer: TextRenderer,
    viewport: Viewport,
    pool: Vec<Buffer>,
}

impl TextPass {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let mut font_system = FontSystem::new();
        let swash = SwashCache::new();
        let cache = Cache::new(device);
        let mut atlas = TextAtlas::new(device, queue, &cache, format);
        let renderer = TextRenderer::new(
            &mut atlas,
            device,
            wgpu::MultisampleState::default(),
            None,
        );
        let viewport = Viewport::new(device, &cache);
        TextPass { font_system, swash, cache, atlas, renderer, viewport, pool: Vec::new() }
    }

    pub fn resize(&mut self, queue: &wgpu::Queue, width: u32, height: u32) {
        self.viewport.update(queue, Resolution { width, height });
    }

    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        items: &[TextItem],
        pass: &mut wgpu::RenderPass,
    ) {
        let Self { font_system, swash, atlas, renderer, viewport, pool, .. } = self;
        while pool.len() < items.len() {
            pool.push(Buffer::new(font_system, Metrics::new(16.0, 20.0)));
        }
        for (buf, item) in pool.iter_mut().zip(items.iter()) {
            buf.set_metrics(Metrics::new(item.size.max(1.0), item.line_h.max(1.2)));
            buf.set_size(None, None);
            let mut attrs = Attrs::new();
            if item.mono {
                attrs = attrs.family(Family::Monospace);
            }
            let text = sanitize(&item.text);
            // cosmic-text panics on some bidi/separator characters found in real source files
            let shaped = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                buf.set_text(&text, &attrs, Shaping::Advanced, None);
                buf.shape_until_scroll(font_system, false);
            }));
            if shaped.is_err() {
                buf.set_text("", &attrs, Shaping::Basic, None);
            }
        }
        let mut areas: Vec<TextArea> = Vec::with_capacity(items.len());
        for (buf, item) in pool.iter().zip(items.iter()) {
            let c = item.color;
            areas.push(TextArea {
                buffer: buf,
                left: item.x,
                top: item.y,
                scale: 1.0,
                bounds: TextBounds {
                    left: item.clip[0] as i32,
                    top: item.clip[1] as i32,
                    right: (item.clip[0] + item.clip[2]) as i32,
                    bottom: (item.clip[1] + item.clip[3]) as i32,
                },
                default_color: Color::rgba(
                    (c[0] * 255.0) as u8,
                    (c[1] * 255.0) as u8,
                    (c[2] * 255.0) as u8,
                    (c[3] * 255.0) as u8,
                ),
                custom_glyphs: &[],
            });
        }
        if let Err(e) = renderer.prepare(device, queue, font_system, atlas, viewport, areas, swash) {
            eprintln!("glyphon prepare error: {e:?}");
        }
        if let Err(e) = renderer.render(atlas, viewport, pass) {
            eprintln!("glyphon render error: {e:?}");
        }
        atlas.trim();
    }
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '\u{000b}' | '\u{000c}' | '\u{0085}' | '\u{2028}' | '\u{2029}' | '\u{200e}' | '\u{200f}'
            | '\u{061c}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' => ' ',
            c => c,
        })
        .collect()
}
