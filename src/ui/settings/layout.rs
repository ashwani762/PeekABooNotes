//! Settings geometry: rows, controls and hit testing.

use super::*;

pub(super) fn client_size(h: HWND) -> (i32, i32) {
    let mut rc: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetClientRect(h, &mut rc) };
    (rc.right.max(1), rc.bottom.max(1))
}

pub(super) fn close_rect(w: &Win) -> RECT {
    let (cw, _) = client_size(w.hwnd);
    let (bw, bh) = ((46.0 * w.s) as i32, (34.0 * w.s) as i32);
    rect(cw - bw, 0, bw, bh)
}

pub(super) fn side_item(w: &Win, i: usize) -> Shape {
    let s = w.s;
    Shape::new(10.0 * s, SIDE_TOP * s + i as f32 * SIDE_ROW * s, (SIDE_W - 20.0) * s, 32.0 * s, 8.0 * s)
}

pub(super) fn content_x(w: &Win) -> (f32, f32) {
    let (cw, _) = client_size(w.hwnd);
    let x0 = (SIDE_W + PAD) * w.s;
    (x0, cw as f32 - x0 - PAD * w.s)
}

pub(super) fn row_h(r: &Row) -> f32 {
    match (&r.ctl, &r.sub) {
        (Ctl::Keys(_), _) => KEY_ROW_H,
        (Ctl::Preview, _) => PREVIEW_H,
        (_, Some(_)) => ROW_SUB_H,
        _ => ROW_H,
    }
}

pub(super) fn place<'a>(w: &Win, groups: &'a [Group]) -> Vec<Item<'a>> {
    let s = w.s;
    let (x0, cw) = content_x(w);
    let mut y = GROUP_TOP * s;
    let mut out = Vec::new();
    for g in groups {
        if let Some(h) = g.header {
            out.push(Item::Header(h, (y + 17.0 * s) as i32));
            y += HEADER_H * s;
        }
        let total: f32 = g.rows.iter().map(|r| row_h(r) * s).sum();
        out.push(Item::Box(Shape::new(x0, y, cw, total, 10.0 * s)));
        for (i, r) in g.rows.iter().enumerate() {
            let h = row_h(r) * s;
            out.push(Item::Row(Placed { row: r, r: rect(x0 as i32, y as i32, cw as i32, h as i32), first: i == 0 }));
            y += h;
        }
        if let Some(f) = &g.footer {
            out.push(Item::Footer(f, (y + 19.0 * s) as i32));
            y += FOOTER_H * s;
        }
        y += GROUP_GAP * s;
    }
    out
}

pub(super) fn seg_metrics(w: &Win, labels: &[&str], r: &RECT) -> (Shape, f32) {
    let s = w.s;
    let widest = labels.iter().map(|l| w.fonts.seg_on.measure(l).0).max().unwrap_or(0) as f32;
    let item = (widest + 28.0 * s).max(46.0 * s);
    let (sw, sh) = (item * labels.len() as f32 + 4.0 * s, 28.0 * s);
    let x = r.right as f32 - INSET * s - sw;
    let y = (r.top + r.bottom) as f32 / 2.0 - sh / 2.0;
    (Shape::new(x, y, sw, sh, 8.0 * s), item)
}

pub(super) fn toggle_shape(w: &Win, r: &RECT) -> Shape {
    let s = w.s;
    let (tw, th) = (TOGGLE.0 * s, TOGGLE.1 * s);
    Shape::new(r.right as f32 - INSET * s - tw, (r.top + r.bottom) as f32 / 2.0 - th / 2.0, tw, th, th / 2.0)
}

pub(super) fn swatch_x(w: &Win, r: &RECT, i: usize) -> (f32, f32) {
    let s = w.s;
    let n = theme::ORDER.len() + 1;
    let total = n as f32 * SWATCH * s + (n - 1) as f32 * SWATCH_GAP * s;
    let x = r.right as f32 - INSET * s - total + i as f32 * (SWATCH + SWATCH_GAP) * s;
    (x, (r.top + r.bottom) as f32 / 2.0 - SWATCH * s / 2.0)
}

pub(super) fn button_shape(w: &Win, label: &str, r: &RECT) -> Shape {
    let s = w.s;
    let bw = w.fonts.button.measure(label).0 as f32 + 28.0 * s;
    let bh = 28.0 * s;
    Shape::new(r.right as f32 - INSET * s - bw, (r.top + r.bottom) as f32 / 2.0 - bh / 2.0, bw, bh, bh / 2.0)
}

/// The language pop-up button: its current choice, a gap and a chevron.
pub(super) fn choice_shape(w: &Win, r: &RECT) -> Shape {
    let s = w.s;
    let bw = w.fonts.button.measure(&language_label()).0 as f32 + 44.0 * s;
    let bh = 28.0 * s;
    Shape::new(r.right as f32 - INSET * s - bw, (r.top + r.bottom) as f32 / 2.0 - bh / 2.0, bw, bh, 8.0 * s)
}

pub(super) fn inside(sh: &Shape, x: i32, y: i32, pad: f32) -> bool {
    let (x, y) = (x as f32, y as f32);
    x >= sh.x - pad && x < sh.x + sh.w + pad && y >= sh.y - pad && y < sh.y + sh.h + pad
}

pub(super) fn hit_test(w: &Win, x: i32, y: i32) -> Hit {
    if pt_in(&close_rect(w), x, y) {
        return Hit::Close;
    }
    let s = w.s;
    if (x as f32) < SIDE_W * s {
        for i in 0..PAGES.len() {
            if inside(&side_item(w, i), x, y, 1.0 * s) {
                return Hit::Page(i);
            }
        }
        return Hit::None;
    }
    let groups = page(w.page);
    for it in place(w, &groups) {
        let Item::Row(p) = it else { continue };
        let Some(k) = p.row.key else { continue };
        if !pt_in(&p.r, x, y) {
            continue;
        }
        return match &p.row.ctl {
            // The whole row flips a switch: a big, forgiving target.
            Ctl::Toggle => Hit::Ctl(k, 0),
            Ctl::Seg(labels) => {
                let (sh, item) = seg_metrics(w, labels, &p.r);
                if !inside(&sh, x, y, 2.0 * s) {
                    return Hit::None;
                }
                let i = ((x as f32 - sh.x - 2.0 * s) / item).floor().clamp(0.0, labels.len() as f32 - 1.0) as usize;
                Hit::Ctl(k, i)
            }
            Ctl::Swatches => {
                for i in 0..=theme::ORDER.len() {
                    let (sx, sy) = swatch_x(w, &p.r, i);
                    if inside(&Shape::new(sx, sy, SWATCH * s, SWATCH * s, 0.0), x, y, SWATCH_GAP * s / 2.0) {
                        return Hit::Ctl(k, i);
                    }
                }
                Hit::None
            }
            Ctl::Button(label) => {
                if inside(&button_shape(w, label, &p.r), x, y, 2.0 * s) {
                    Hit::Ctl(k, 0)
                } else {
                    Hit::None
                }
            }
            Ctl::Choice => {
                if inside(&choice_shape(w, &p.r), x, y, 2.0 * s) {
                    Hit::Ctl(k, 0)
                } else {
                    Hit::None
                }
            }
            Ctl::Keys(_) | Ctl::Preview => Hit::None,
        };
    }
    Hit::None
}
