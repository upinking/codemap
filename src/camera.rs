pub struct Camera {
    pub cx: f64,
    pub cy: f64,
    pub scale: f64,
    pub tx: f64,
    pub ty: f64,
    pub tscale: f64,
    pub min_scale: f64,
    pub max_scale: f64,
}

impl Camera {
    pub fn new() -> Self {
        Camera { cx: 0.0, cy: 0.0, scale: 1.0, tx: 0.0, ty: 0.0, tscale: 1.0, min_scale: 1e-4, max_scale: 6000.0 }
    }

    pub fn fit(&mut self, world_w: f64, world_h: f64, sw: f64, sh: f64) {
        let s = (sw / world_w).min(sh / world_h) * 0.92;
        self.scale = s;
        self.tscale = s;
        self.cx = world_w / 2.0;
        self.cy = world_h / 2.0;
        self.tx = self.cx;
        self.ty = self.cy;
        self.min_scale = s * 0.3;
        self.max_scale = 8000.0;
    }

    pub fn zoom_at(&mut self, sx: f64, sy: f64, sw: f64, sh: f64, factor: f64) {
        let old = self.tscale;
        let new = (old * factor).clamp(self.min_scale, self.max_scale);
        if (new - old).abs() < 1e-15 {
            return;
        }
        let wx = self.tx + (sx - sw / 2.0) / old;
        let wy = self.ty + (sy - sh / 2.0) / old;
        self.tscale = new;
        self.tx = wx - (sx - sw / 2.0) / new;
        self.ty = wy - (sy - sh / 2.0) / new;
    }

    pub fn pan_px(&mut self, dx: f64, dy: f64) {
        self.tx -= dx / self.tscale;
        self.ty -= dy / self.tscale;
        self.cx = self.tx;
        self.cy = self.ty;
    }

    pub fn update(&mut self, dt: f64) {
        let k = 1.0 - (-dt * 22.0).exp();
        self.cx += (self.tx - self.cx) * k;
        self.cy += (self.ty - self.cy) * k;
        self.scale += (self.tscale - self.scale) * k;
    }

    pub fn view(&self, sw: f64, sh: f64) -> crate::render::View {
        crate::render::View { cx: self.cx, cy: self.cy, scale: self.scale, sw, sh }
    }
}
