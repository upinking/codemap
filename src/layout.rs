use crate::index::{Node, Scene};

pub fn squarify(scene: &mut Scene, root: usize) {
    let total = scene.nodes[root].weight.max(1.0);
    let w = (total * 1.7f64).sqrt();
    let h = total / w;
    scene.nodes[root].rect = [0.0, 0.0, w as f32, h as f32];
    layout_children(scene, root, 0.0, 0.0, w, h);
}

fn layout_children(scene: &mut Scene, parent: usize, x: f64, y: f64, w: f64, h: f64) {
    let count = scene.nodes[parent].children_count as usize;
    if count == 0 || w <= 0.0 || h <= 0.0 {
        return;
    }
    let start = scene.nodes[parent].children_start as usize;
    let kids: Vec<usize> = scene.children_ids[start..start + count].iter().map(|&i| i as usize).collect();

    let (ix, iy, iw, ih) = inner_rect(x, y, w, h);
    if iw <= 0.0 || ih <= 0.0 {
        for &i in &kids {
            scene.nodes[i].rect = [ix as f32, iy as f32, 0.0, 0.0];
        }
        return;
    }

    let weight_sum: f64 = kids.iter().map(|&i| scene.nodes[i].weight).sum();
    if std::env::var("CODEMAP_DUMP").is_ok() {
        eprintln!(
            "layout parent={} rect=[{:.2},{:.2},{:.2},{:.2}] inner=[{:.2},{:.2},{:.2},{:.2}] k={:.4} wsum={:.1} children={}..{}",
            crate::index::node_name(scene, &scene.nodes[parent]),
            x, y, w, h, ix, iy, iw, ih,
            (iw * ih) / weight_sum.max(1.0),
            weight_sum,
            start,
            start + count
        );
    }
    if weight_sum <= 0.0 {
        for &i in &kids {
            scene.nodes[i].rect = [ix as f32, iy as f32, 0.0, 0.0];
        }
        return;
    }
    let k = (iw * ih) / weight_sum;

    let mut rem = [ix, iy, iw, ih];
    let mut idx = 0usize;

    while idx < kids.len() {
        let side = rem[2].min(rem[3]);
        if side <= 1e-9 {
            for &i in &kids[idx..] {
                scene.nodes[i].rect = [rem[0] as f32, rem[1] as f32, 0.0, 0.0];
            }
            break;
        }

        let mut row: Vec<usize> = vec![kids[idx]];
        let mut row_sum = scene.nodes[kids[idx]].weight * k;
        let mut worst = worst_ratio(scene, &row, row_sum, side, k);
        while idx + row.len() < kids.len() {
            let nxt = kids[idx + row.len()];
            let new_sum = row_sum + scene.nodes[nxt].weight * k;
            let mut cand = row.clone();
            cand.push(nxt);
            let w2 = worst_ratio(scene, &cand, new_sum, side, k);
            if w2 <= worst {
                row = cand;
                row_sum = new_sum;
                worst = w2;
            } else {
                break;
            }
        }

        let thick = row_sum / side;
        let width_shorter = rem[2] <= rem[3];
        let mut off = 0.0;
        for &c in &row {
            let len = scene.nodes[c].weight * k / thick;
            let (cx, cy, cw, ch) = if width_shorter {
                (rem[0] + off, rem[1], len, thick)
            } else {
                (rem[0], rem[1] + off, thick, len)
            };
            scene.nodes[c].rect = [cx as f32, cy as f32, cw as f32, ch as f32];
            if scene.nodes[c].children_count > 0 {
                layout_children(scene, c, cx, cy, cw, ch);
            }
            off += len;
        }

        rem = if width_shorter {
            [rem[0], rem[1] + thick, rem[2], (rem[3] - thick).max(0.0)]
        } else {
            [rem[0] + thick, rem[1], (rem[2] - thick).max(0.0), rem[3]]
        };
        idx += row.len();
    }
}

fn inner_rect(x: f64, y: f64, w: f64, h: f64) -> (f64, f64, f64, f64) {
    let ix = x + w * 0.014;
    let iy = y + h * 0.055;
    let iw = w * 0.972;
    let ih = h * 0.931;
    (ix, iy, iw.max(0.0), ih.max(0.0))
}

fn worst_ratio(scene: &Scene, row: &[usize], row_sum: f64, side: f64, k: f64) -> f64 {
    let q = side * side / (row_sum * row_sum);
    let mut worst = 0.0f64;
    for &i in row {
        let a = scene.nodes[i].weight * k;
        let r1 = a * q;
        let r2 = 1.0 / r1;
        worst = worst.max(r1.max(r2));
    }
    worst
}

pub fn node_rect(_n: &Node) {}
