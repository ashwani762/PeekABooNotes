// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! A tiny software canvas for per-pixel-alpha layered windows.
//!
//! Shapes are rasterised with an anti-aliased rounded-rectangle SDF straight into a
//! premultiplied BGRA DIB. Text goes through DirectWrite (see `dw`).

use std::ptr::null_mut;

use windows_sys::Win32::Foundation::{HWND, POINT, RECT, SIZE};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateIconIndirect, GetClientRect, HICON, ICONINFO, ULW_ALPHA, UpdateLayeredWindow,
};

pub use crate::win::render::dw::Font;
use crate::win::render::dw;

struct Dib {
    dc: HDC,
    bmp: HBITMAP,
    old: HGDIOBJ,
    px: *mut u32,
}

impl Dib {
    fn new(w: i32, h: i32) -> Dib {
        unsafe {
            let mut bi: BITMAPINFO = std::mem::zeroed();
            bi.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
            bi.bmiHeader.biWidth = w;
            bi.bmiHeader.biHeight = -h;
            bi.bmiHeader.biPlanes = 1;
            bi.bmiHeader.biBitCount = 32;
            bi.bmiHeader.biCompression = BI_RGB;
            let mut bits = null_mut();
            let bmp = CreateDIBSection(null_mut(), &bi, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
            let dc = CreateCompatibleDC(null_mut());
            let old = SelectObject(dc, bmp);
            Dib { dc, bmp, old, px: bits as *mut u32 }
        }
    }

    fn release(&mut self) -> HBITMAP {
        unsafe {
            SelectObject(self.dc, self.old);
            DeleteDC(self.dc);
        }
        self.dc = null_mut();
        std::mem::replace(&mut self.bmp, null_mut())
    }
}

impl Drop for Dib {
    fn drop(&mut self) {
        if !self.dc.is_null() {
            let b = self.release();
            unsafe { DeleteObject(b) };
        }
    }
}

/// A rounded rectangle, optionally rotated by `angle` radians around (`px`, `py`).
#[derive(Clone, Copy, Debug)]
pub struct Shape {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub r: f32,
    pub angle: f32,
    pub px: f32,
    pub py: f32,
}

impl Shape {
    pub fn new(x: f32, y: f32, w: f32, h: f32, r: f32) -> Shape {
        Shape { x, y, w, h, r, angle: 0.0, px: x, py: y }
    }

    pub fn rotated(mut self, angle: f32, px: f32, py: f32) -> Shape {
        self.angle = angle;
        self.px = px;
        self.py = py;
        self
    }

    pub fn inset(mut self, d: f32) -> Shape {
        self.x += d;
        self.y += d;
        self.w -= 2.0 * d;
        self.h -= 2.0 * d;
        self.r = (self.r - d).max(0.0);
        self
    }
}

pub struct Canvas {
    pub w: i32,
    pub h: i32,
    img: Dib,
    scratch: Option<Dib>,
    batch: Option<Vec<dw::Op>>,
}

impl Canvas {
    pub fn new(w: i32, h: i32) -> Canvas {
        let (w, h) = (w.max(1), h.max(1));
        Canvas { w, h, img: Dib::new(w, h), scratch: None, batch: None }
    }

    fn px(&mut self) -> &mut [u32] {
        unsafe { std::slice::from_raw_parts_mut(self.img.px, (self.w * self.h) as usize) }
    }

    /// Fill with a premultiplied ARGB value. `0x01000000` gives an invisible but hit-testable surface.
    pub fn clear(&mut self, argb: u32) {
        unsafe { GdiFlush() };
        self.px().fill(argb);
    }

    /// Overwrite a rectangle with a raw premultiplied ARGB value (e.g. the alpha-1 hover zone).
    pub fn fill_raw(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, argb: u32) {
        let (w, h) = (self.w, self.h);
        let px = self.px();
        for y in y0.max(0)..y1.min(h) {
            let row = (y * w) as usize;
            px[row + x0.max(0) as usize..row + x1.min(w).max(x0.max(0)) as usize].fill(argb);
        }
    }

    #[inline]
    fn blend(p: &mut u32, rgb: u32, a: f32) {
        if a <= 0.0 {
            return;
        }
        let a = a.min(1.0);
        let d = *p;
        let inv = 1.0 - a;
        let ch = |s: u32, sh: u32| -> u32 {
            let sc = ((s >> sh) & 0xFF) as f32 * a;
            let dc = ((d >> sh) & 0xFF) as f32 * inv;
            ((sc + dc + 0.5) as u32).min(255)
        };
        let da = ((d >> 24) & 0xFF) as f32;
        let oa = ((a * 255.0 + da * inv + 0.5) as u32).min(255);
        *p = (oa << 24) | (ch(rgb, 16) << 16) | (ch(rgb, 8) << 8) | ch(rgb, 0);
    }

    /// Anti-aliased rounded rectangle. Colour is 0xRRGGBB, alpha in 0..1.
    #[allow(clippy::too_many_arguments)]
    pub fn rrect(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, rgb: u32, alpha: f32) {
        self.shape(&Shape::new(x, y, w, h, r), rgb, alpha, 1.0, 0.0);
    }

    /// General rounded rectangle: optional rotation (radians, around `pivot`), an edge
    /// `feather` in pixels (1 = crisp anti-aliasing, larger = soft, for shadows), and an
    /// optional inner `stroke` width (0 = filled).
    #[allow(clippy::too_many_arguments)]
    pub fn shape(&mut self, s: &Shape, rgb: u32, alpha: f32, feather: f32, stroke: f32) {
        if s.w <= 0.0 || s.h <= 0.0 || alpha <= 0.002 {
            return;
        }
        let (sin, cos) = (-s.angle).sin_cos();
        let (hx, hy) = (s.w / 2.0, s.h / 2.0);
        let (cx, cy) = (s.x + hx, s.y + hy);
        let r = s.r.min(hx).min(hy).max(0.0);
        let grow = feather.max(1.0);
        // Screen-space bounds of the rotated, feathered rectangle.
        let corners = [(s.x - grow, s.y - grow), (s.x + s.w + grow, s.y - grow), (s.x - grow, s.y + s.h + grow), (s.x + s.w + grow, s.y + s.h + grow)];
        let (mut bx0, mut by0, mut bx1, mut by1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        let (fs, fc) = s.angle.sin_cos();
        for (x, y) in corners {
            // Forward rotation (the inverse of how pixels are sampled below).
            let (dx, dy) = (x - s.px, y - s.py);
            let (fx, fy) = (s.px + dx * fc - dy * fs, s.py + dx * fs + dy * fc);
            bx0 = bx0.min(fx);
            by0 = by0.min(fy);
            bx1 = bx1.max(fx);
            by1 = by1.max(fy);
        }
        let (cw, ch) = (self.w, self.h);
        let x0 = (bx0.floor() as i32).max(0);
        let y0 = (by0.floor() as i32).max(0);
        let x1 = (bx1.ceil() as i32 + 1).min(cw);
        let y1 = (by1.ceil() as i32 + 1).min(ch);
        if x0 >= x1 || y0 >= y1 {
            return; // entirely off-canvas
        }
        let rotated = s.angle.abs() > 1e-4;
        let filled = !rotated && feather <= 1.0 && stroke <= 0.0;
        let opaque = 0xFF00_0000 | (rgb & 0x00FF_FFFF);
        let px = self.px();
        for py in y0..y1 {
            let row = (py * cw) as usize;
            // Fast path for filled shapes: rows clear of the corners have a uniform interior.
            if filled && ((py as f32 + 0.5) - cy).abs() < hy - r - 1.0 {
                let a = ((s.x + 1.0).ceil() as i32).clamp(x0, x1);
                let b = ((s.x + s.w - 1.0).floor() as i32).clamp(a, x1);
                if alpha >= 0.999 {
                    px[row + a as usize..row + b as usize].fill(opaque);
                } else {
                    for p in &mut px[row + a as usize..row + b as usize] {
                        Self::blend(p, rgb, alpha);
                    }
                }
                for pxx in (x0..a).chain(b..x1) {
                    let sx = pxx as f32 + 0.5;
                    let qx = (sx - cx).abs() - hx;
                    let cov = (0.5 - qx).clamp(0.0, 1.0);
                    if cov > 0.0 {
                        Self::blend(&mut px[row + pxx as usize], rgb, alpha * cov);
                    }
                }
                continue;
            }
            for pxx in x0..x1 {
                let (mut sx, mut sy) = (pxx as f32 + 0.5, py as f32 + 0.5);
                if rotated {
                    let (dx, dy) = (sx - s.px, sy - s.py);
                    sx = s.px + dx * cos - dy * sin;
                    sy = s.py + dx * sin + dy * cos;
                }
                let qx = (sx - cx).abs() - (hx - r);
                let qy = (sy - cy).abs() - (hy - r);
                let d = if qx > 0.0 && qy > 0.0 { (qx * qx + qy * qy).sqrt() - r } else { qx.max(qy) - r };
                let cov = if feather <= 1.0 {
                    let outer = (0.5 - d).clamp(0.0, 1.0);
                    if stroke > 0.0 { outer * (d + stroke + 0.5).clamp(0.0, 1.0) } else { outer }
                } else {
                    let t = ((d / feather) + 0.5).clamp(0.0, 1.0);
                    1.0 - t * t * (3.0 - 2.0 * t)
                };
                if cov > 0.0 {
                    Self::blend(&mut px[row + pxx as usize], rgb, alpha * cov);
                }
            }
        }
    }

    /// A complete card in one pass: two-layer shadow, fill and 1px hairline. Shadow work is
    /// skipped under opaque fill, which is most of the card's area.
    #[allow(clippy::too_many_arguments)]
    pub fn card(&mut self, s: &Shape, fill: u32, fill_a: f32, edge: u32, edge_a: f32, shadow: f32, scale: f32) {
        if s.w <= 0.0 || s.h <= 0.0 || fill_a <= 0.002 {
            return;
        }
        let (amb_dy, amb_f, key_dy, key_f) = (8.0 * scale, 22.0 * scale, 1.0 * scale, 3.0 * scale);
        let (sin, cos) = (-s.angle).sin_cos();
        let (fs, fc) = s.angle.sin_cos();
        let (hx, hy) = (s.w / 2.0, s.h / 2.0);
        let (cx, cy) = (s.x + hx, s.y + hy);
        let r = s.r.min(hx).min(hy).max(0.0);
        let grow = if shadow > 0.0 { amb_f + amb_dy } else { 1.0 };
        let corners = [(s.x - grow, s.y - grow), (s.x + s.w + grow, s.y - grow), (s.x - grow, s.y + s.h + grow), (s.x + s.w + grow, s.y + s.h + grow)];
        let (mut bx0, mut by0, mut bx1, mut by1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for (x, y) in corners {
            let (dx, dy) = (x - s.px, y - s.py);
            let (fx, fy) = (s.px + dx * fc - dy * fs, s.py + dx * fs + dy * fc);
            bx0 = bx0.min(fx);
            by0 = by0.min(fy);
            bx1 = bx1.max(fx);
            by1 = by1.max(fy);
        }
        let (cw, ch) = (self.w, self.h);
        let x0 = (bx0.floor() as i32).max(0);
        let y0 = (by0.floor() as i32).max(0);
        let x1 = (bx1.ceil() as i32 + 1).min(cw);
        let y1 = (by1.ceil() as i32 + 1).min(ch);
        if x0 >= x1 || y0 >= y1 {
            return; // entirely off-canvas
        }
        let rotated = s.angle.abs() > 1e-4;
        let sdf = |lx: f32, ly: f32| -> f32 {
            let qx = (lx - cx).abs() - (hx - r);
            let qy = (ly - cy).abs() - (hy - r);
            if qx > 0.0 && qy > 0.0 { (qx * qx + qy * qy).sqrt() - r } else { qx.max(qy) - r }
        };
        let soft = |d: f32, f: f32| -> f32 {
            let t = (d / f + 0.5).clamp(0.0, 1.0);
            1.0 - t * t * (3.0 - 2.0 * t)
        };
        let opaque = 0xFF00_0000 | (fill & 0x00FF_FFFF);
        let solid = fill_a >= 0.999;
        let px = self.px();
        for py in y0..y1 {
            let row = (py * cw) as usize;
            // Fast path: on rows clear of the rounded corners, the solid interior is a
            // plain span fill; only the edges and shadow band need per-pixel work.
            let ly0 = py as f32 + 0.5;
            let span = if solid && !rotated && (ly0 - cy).abs() < hy - r - 2.0 {
                let a = ((s.x + 2.0).ceil() as i32).clamp(x0, x1);
                let b = ((s.x + s.w - 2.0).floor() as i32).clamp(a, x1);
                Some((a, b))
            } else {
                None
            };
            let mut pxx = x0;
            while pxx < x1 {
                if let Some((a, b)) = span
                    && pxx == a && b > a {
                        px[row + a as usize..row + b as usize].fill(opaque);
                        pxx = b;
                        continue;
                    }
                let col = pxx;
                pxx += 1;
                let (mut lx, mut ly) = (col as f32 + 0.5, py as f32 + 0.5);
                if rotated {
                    let (dx, dy) = (lx - s.px, ly - s.py);
                    lx = s.px + dx * cos - dy * sin;
                    ly = s.py + dx * sin + dy * cos;
                }
                let d = sdf(lx, ly);
                let p = &mut px[row + col as usize];
                if d < -1.5 && solid {
                    *p = opaque; // deep inside: no shadow, no edge, no blending
                    continue;
                }
                if shadow > 0.0 && d > -1.0 {
                    let a = soft(sdf(lx, ly - amb_dy), amb_f) * 0.16 * shadow;
                    let k = soft(sdf(lx, ly - key_dy), key_f) * 0.10 * shadow;
                    let sa = 1.0 - (1.0 - a) * (1.0 - k);
                    if sa > 0.002 {
                        Self::blend(p, 0x000000, sa);
                    }
                }
                let cov = (0.5 - d).clamp(0.0, 1.0);
                if cov > 0.0 {
                    Self::blend(p, fill, fill_a * cov);
                    if edge_a > 0.0 {
                        let e = cov * (d + 1.5).clamp(0.0, 1.0);
                        if e > 0.0 {
                            Self::blend(p, edge, edge_a * e);
                        }
                    }
                }
            }
        }
    }

    /// Two-layer shadow: a wide soft ambient shadow plus a tight contact shadow.
    pub fn soft_shadow(&mut self, s: &Shape, scale: f32, strength: f32) {
        let mut amb = *s;
        amb.y += 8.0 * scale;
        self.shape(&amb, 0x000000, 0.16 * strength, 22.0 * scale, 0.0);
        let mut key = *s;
        key.y += 1.0 * scale;
        self.shape(&key, 0x000000, 0.10 * strength, 3.0 * scale, 0.0);
    }

    /// Anti-aliased ring arc starting at 12 o'clock, clockwise, covering `frac` of a turn.
    #[allow(clippy::too_many_arguments)]
    pub fn ring(&mut self, cx: f32, cy: f32, r: f32, thickness: f32, frac: f32, rgb: u32, alpha: f32) {
        if frac <= 0.0 || alpha <= 0.0 {
            return;
        }
        let (cw, ch) = (self.w, self.h);
        let ext = r + thickness;
        let x0 = ((cx - ext).floor() as i32).max(0);
        let y0 = ((cy - ext).floor() as i32).max(0);
        let x1 = ((cx + ext).ceil() as i32 + 1).min(cw);
        let y1 = ((cy + ext).ceil() as i32 + 1).min(ch);
        let limit = frac.min(1.0) * std::f32::consts::TAU;
        let px = self.px();
        for y in y0..y1 {
            for x in x0..x1 {
                let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
                let d = ((dx * dx + dy * dy).sqrt() - r).abs();
                let cov = (thickness / 2.0 + 0.5 - d).clamp(0.0, 1.0);
                if cov <= 0.0 {
                    continue;
                }
                let mut ang = dx.atan2(-dy);
                if ang < 0.0 {
                    ang += std::f32::consts::TAU;
                }
                if frac < 1.0 && ang > limit {
                    continue;
                }
                Self::blend(&mut px[(y * cw + x) as usize], rgb, alpha * cov);
            }
        }
    }

    fn target(&mut self) -> dw::Target<'_> {
        if self.scratch.is_none() {
            self.scratch = Some(Dib::new(self.w, self.h));
        }
        let n = (self.w * self.h) as usize;
        let sc = self.scratch.as_ref().unwrap();
        unsafe {
            dw::Target {
                scratch_dc: sc.dc,
                scratch: std::slice::from_raw_parts_mut(sc.px, n),
                img: std::slice::from_raw_parts_mut(self.img.px, n),
                w: self.w,
                h: self.h,
                batch: self.batch.as_mut(),
            }
        }
    }

    /// Record text instead of drawing it, until `flush`. Use when nothing drawn afterwards
    /// overlaps the text (one Direct2D pass per frame instead of one per string).
    pub fn begin_batch(&mut self) {
        if self.batch.is_none() {
            self.batch = Some(Vec::new());
        }
    }

    pub fn batching(&self) -> bool {
        self.batch.is_some()
    }

    /// Draw everything recorded since `begin_batch`.
    pub fn flush(&mut self) {
        if let Some(ops) = self.batch.take() {
            let mut t = self.target();
            dw::render(&mut t, &ops);
        }
    }

    /// Text in a box (DirectWrite). `flags`: DT_LEFT/CENTER/RIGHT, DT_VCENTER/BOTTOM,
    /// DT_WORDBREAK, DT_END_ELLIPSIS.
    pub fn text(&mut self, s: &str, f: &Font, r: RECT, rgb: u32, alpha: f32, flags: u32) {
        dw::text(&mut self.target(), s, f, r, None, rgb, alpha, flags);
    }

    /// One line on a baseline; returns its advance width.
    #[allow(clippy::too_many_arguments)]
    pub fn line(&mut self, s: &str, f: &Font, x: i32, baseline: i32, w: i32, clip: Option<RECT>, rgb: u32, alpha: f32, align: u32) -> i32 {
        dw::line(&mut self.target(), s, f, x, baseline, w, clip, rgb, alpha, align)
    }

    /// `text` with an optional clip and full opacity (call-site convenience).
    pub fn text_ct(&mut self, s: &str, f: &Font, r: RECT, clip: Option<RECT>, rgb: u32, flags: u32) {
        dw::text(&mut self.target(), s, f, r, clip, rgb, 1.0, flags);
    }

    /// `text`, but nothing drawn below `bottom`.
    #[allow(clippy::too_many_arguments)]
    pub fn text_above(&mut self, s: &str, f: &Font, r: RECT, bottom: i32, rgb: u32, alpha: f32, flags: u32) {
        dw::text_above(&mut self.target(), s, f, r, bottom, rgb, alpha, flags);
    }

    /// `line` at full opacity.
    #[allow(clippy::too_many_arguments)]
    pub fn line_ct(&mut self, s: &str, f: &Font, x: i32, baseline: i32, w: i32, clip: Option<RECT>, rgb: u32, align: u32) -> i32 {
        self.line(s, f, x, baseline, w, clip, rgb, 1.0, align)
    }

    /// One line laid out at `origin` and rotated `angle` degrees clockwise about `pivot`.
    #[allow(clippy::too_many_arguments)]
    pub fn rotated(&mut self, s: &str, f: &Font, origin: (f32, f32), angle: f32, pivot: (f32, f32), clip: RECT, rgb: u32, alpha: f32) {
        dw::rotated(&mut self.target(), s, f, origin, angle, pivot, clip, rgb, alpha);
    }

    /// Copy the pixels of `r` out (for caching expensive, rarely-changing regions).
    /// The pixels in `r` as a picture (None unless `r` lies wholly on the canvas).
    pub fn snapshot(&mut self, r: RECT) -> Option<Image> {
        if r.left < 0 || r.top < 0 || r.right > self.w || r.bottom > self.h || r.right <= r.left || r.bottom <= r.top {
            return None;
        }
        Some(Image { w: r.right - r.left, h: r.bottom - r.top, px: self.copy_out(r) })
    }

    pub fn copy_out(&mut self, r: RECT) -> Vec<u32> {
        let (w, h) = (self.w, self.h);
        let r = RECT { left: r.left.max(0), top: r.top.max(0), right: r.right.min(w), bottom: r.bottom.min(h) };
        let px = self.px();
        let mut out = Vec::with_capacity(((r.right - r.left).max(0) * (r.bottom - r.top).max(0)) as usize);
        for y in r.top..r.bottom {
            let row = (y * w) as usize;
            out.extend_from_slice(&px[row + r.left as usize..row + r.right as usize]);
        }
        out
    }

    /// Paste pixels previously taken with `copy_out` from the same rect.
    pub fn copy_in(&mut self, r: RECT, src: &[u32]) {
        let (w, h) = (self.w, self.h);
        let r = RECT { left: r.left.max(0), top: r.top.max(0), right: r.right.min(w), bottom: r.bottom.min(h) };
        let rw = (r.right - r.left).max(0) as usize;
        if src.len() != rw * (r.bottom - r.top).max(0) as usize {
            return;
        }
        let px = self.px();
        for (k, y) in (r.top..r.bottom).enumerate() {
            let row = (y * w) as usize;
            px[row + r.left as usize..row + r.right as usize].copy_from_slice(&src[k * rw..(k + 1) * rw]);
        }
    }

    /// Restore full alpha inside `r` only (opaque interior of a translucent-edged surface).
    pub fn make_opaque_rect(&mut self, r: RECT) {
        unsafe { GdiFlush() };
        let (w, h) = (self.w, self.h);
        let px = self.px();
        for y in r.top.max(0)..r.bottom.min(h) {
            let row = (y * w) as usize;
            for p in &mut px[row + r.left.max(0) as usize..row + r.right.min(w).max(r.left.max(0)) as usize] {
                *p |= 0xFF00_0000;
            }
        }
    }

    /// Multiply the whole (premultiplied) surface by `a` — a cheap global fade.
    pub fn scale_alpha(&mut self, a: f32) {
        let k = (a.clamp(0.0, 1.0) * 256.0) as u32;
        for p in self.px().iter_mut() {
            let v = *p;
            let ch = |sh: u32| ((((v >> sh) & 0xFF) * k) >> 8) << sh;
            *p = ch(24) | ch(16) | ch(8) | ch(0);
        }
    }

    /// Blend a rectangle toward `rgb`, keeping `a` of what's there: fades a whole region
    /// (text included) in one pass. Each pixel keeps its own opacity, so a see-through
    /// surface (a card mid-flight) fades without going solid, or dark where it's faint.
    pub fn fade_rect(&mut self, r: RECT, rgb: u32, a: f32) {
        if a >= 0.999 {
            return;
        }
        let k = (a.clamp(0.0, 1.0) * 256.0) as u32;
        let (w, h) = (self.w, self.h);
        let px = self.px();
        for y in r.top.max(0)..r.bottom.min(h) {
            let row = (y * w) as usize;
            for p in &mut px[row + r.left.max(0) as usize..row + r.right.min(w).max(r.left.max(0)) as usize] {
                let v = *p;
                let alpha = v >> 24;
                let ch = |sh: u32| {
                    // Pixels are premultiplied: the target colour is too, by this pixel's alpha.
                    let (src, dst) = ((v >> sh) & 0xFF, ((rgb >> sh) & 0xFF) * alpha / 255);
                    ((src * k + dst * (256 - k)) >> 8) << sh
                };
                *p = (v & 0xFF00_0000) | ch(16) | ch(8) | ch(0);
            }
        }
    }

    /// An anti-aliased disc whose colour sweeps around the centre through `stops`
    /// (a colour wheel), blended over an opaque canvas.
    pub fn conic_disc(&mut self, cx: f32, cy: f32, r: f32, stops: &[u32]) {
        if stops.is_empty() || r <= 0.0 {
            return;
        }
        let (w, h) = (self.w, self.h);
        let n = stops.len() as f32;
        let ch = |c: u32, sh: u32| ((c >> sh) & 0xFF) as f32;
        let (y0, y1) = (((cy - r - 1.0).floor() as i32).max(0), ((cy + r + 1.0).ceil() as i32).min(h));
        let (x0, x1) = (((cx - r - 1.0).floor() as i32).max(0), ((cx + r + 1.0).ceil() as i32).min(w));
        let px = self.px();
        for y in y0..y1 {
            for x in x0..x1 {
                let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
                let cov = (r - (dx * dx + dy * dy).sqrt() + 0.5).clamp(0.0, 1.0);
                if cov <= 0.0 {
                    continue;
                }
                // Angle from 12 o'clock, clockwise, as a position along the stops.
                let t = (dx.atan2(-dy) / std::f32::consts::TAU).rem_euclid(1.0) * n;
                let (i, f) = (t.floor() as usize % stops.len(), t.fract());
                let (a, b) = (stops[i], stops[(i + 1) % stops.len()]);
                let i = (y * w + x) as usize;
                let dst = px[i];
                let mut out = 0xFF00_0000u32;
                for sh in [16u32, 8, 0] {
                    let src = ch(a, sh) + (ch(b, sh) - ch(a, sh)) * f;
                    let v = ch(dst, sh) + (src - ch(dst, sh)) * cov;
                    out |= (v.round().clamp(0.0, 255.0) as u32) << sh;
                }
                px[i] = out;
            }
        }
    }

    /// Restore full alpha after GDI drawing (opaque canvases only).
    pub fn make_opaque(&mut self) {
        unsafe { GdiFlush() };
        for p in self.px().iter_mut() {
            *p |= 0xFF00_0000;
        }
    }

    /// Copy to a DC (normal, non-layered windows).
    pub fn blit(&self, hdc: HDC, x: i32, y: i32) {
        unsafe {
            GdiFlush();
            BitBlt(hdc, x, y, self.w, self.h, self.img.dc, 0, 0, SRCCOPY);
        }
    }

    pub fn present(&self, hwnd: HWND, x: i32, y: i32) {
        unsafe {
            GdiFlush();
            let dst = POINT { x, y };
            let size = SIZE { cx: self.w, cy: self.h };
            let src = POINT { x: 0, y: 0 };
            let bf = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: 255,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            UpdateLayeredWindow(hwnd, null_mut(), &dst, &size, self.img.dc, &src, 0, &bf, ULW_ALPHA);
        }
    }

    /// Hand over the pixels as a standalone 32-bit bitmap (for icons, menus, image lists).
    pub fn into_bitmap(mut self) -> HBITMAP {
        unsafe { GdiFlush() };
        self.img.release()
    }

    /// Draw `img` with its top-left at (x, y), only inside the rounded rectangle `clip`
    /// (unrotated; anti-aliased at its edge), at `alpha`.
    pub fn image(&mut self, img: &Image, x: f32, y: f32, clip: &Shape, alpha: f32) {
        if alpha <= 0.002 || clip.w <= 0.0 || clip.h <= 0.0 {
            return;
        }
        let (hx, hy) = (clip.w / 2.0, clip.h / 2.0);
        let (cx, cy) = (clip.x + hx, clip.y + hy);
        let r = clip.r.min(hx).min(hy).max(0.0);
        let (ox, oy) = (x.round() as i32, y.round() as i32);
        let x0 = (clip.x.floor() as i32).max(ox).max(0);
        let y0 = (clip.y.floor() as i32).max(oy).max(0);
        let x1 = ((clip.x + clip.w).ceil() as i32).min(ox + img.w).min(self.w);
        let y1 = ((clip.y + clip.h).ceil() as i32).min(oy + img.h).min(self.h);
        let cw = self.w;
        let px = self.px();
        for py in y0..y1 {
            let (row, src_row) = ((py * cw) as usize, ((py - oy) * img.w) as usize);
            for pxx in x0..x1 {
                let (sx, sy) = (pxx as f32 + 0.5, py as f32 + 0.5);
                let qx = (sx - cx).abs() - (hx - r);
                let qy = (sy - cy).abs() - (hy - r);
                let d = if qx > 0.0 && qy > 0.0 { (qx * qx + qy * qy).sqrt() - r } else { qx.max(qy) - r };
                let a = alpha * (0.5 - d).clamp(0.0, 1.0);
                if a <= 0.0 {
                    continue;
                }
                // Both premultiplied; the picture is opaque.
                let (s, dst) = (img.px[src_row + (pxx - ox) as usize], px[row + pxx as usize]);
                let k = (a * 256.0) as u32;
                let ch = |sh: u32| ((((s >> sh) & 0xFF) * k + ((dst >> sh) & 0xFF) * (256 - k)) >> 8) << sh;
                px[row + pxx as usize] = ch(24) | ch(16) | ch(8) | ch(0);
            }
        }
    }
}

/// A picture of a window, for animating it as it opens or closes.
pub struct Image {
    pub w: i32,
    pub h: i32,
    px: Vec<u32>,
}

#[link(name = "user32")]
unsafe extern "system" {
    fn PrintWindow(hwnd: HWND, hdc: HDC, flags: u32) -> i32;
}

/// What `hwnd` (children and all) looks like right now, as Windows composes it: painted
/// first if anything is pending. None if Windows can't provide it.
pub fn capture(hwnd: HWND) -> Option<Image> {
    const PW_CLIENTONLY: u32 = 1;
    const PW_RENDERFULLCONTENT: u32 = 2;
    unsafe {
        let mut rc: RECT = std::mem::zeroed();
        GetClientRect(hwnd, &mut rc);
        let (w, h) = (rc.right, rc.bottom);
        if w <= 0 || h <= 0 {
            return None;
        }
        RedrawWindow(hwnd, std::ptr::null(), null_mut(), RDW_INVALIDATE | RDW_UPDATENOW | RDW_ALLCHILDREN);
        let dib = Dib::new(w, h);
        if PrintWindow(hwnd, dib.dc, PW_CLIENTONLY | PW_RENDERFULLCONTENT) == 0 {
            return None;
        }
        GdiFlush();
        let px = std::slice::from_raw_parts(dib.px, (w * h) as usize).iter().map(|p| p | 0xFF00_0000).collect();
        Some(Image { w, h, px })
    }
}

/// A GDI font for native controls (the search EDIT), ClearType quality.
pub fn gdi_font(px: i32, weight: i32, face: &str) -> HFONT {
    let f = crate::win::util::wide(face);
    unsafe {
        CreateFontW(
            -px,
            0,
            0,
            0,
            weight,
            0,
            0,
            0,
            DEFAULT_CHARSET as u32,
            OUT_TT_PRECIS as u32,
            CLIP_DEFAULT_PRECIS as u32,
            CLEARTYPE_QUALITY as u32,
            (DEFAULT_PITCH | FF_DONTCARE) as u32,
            f.as_ptr(),
        )
    }
}

/// The app icon: a little sticky note with a folded corner and three lines.
pub fn app_icon(size: i32) -> HICON {
    let mut c = Canvas::new(size, size);
    c.clear(0);
    let s = size as f32 / 32.0;
    c.rrect(3.0 * s, 3.0 * s, 26.0 * s, 26.0 * s, 6.0 * s, 0xC99A1E, 1.0);
    c.rrect(4.0 * s, 4.0 * s, 24.0 * s, 24.0 * s, 5.0 * s, 0xF7CF4E, 1.0);
    for i in 0..3 {
        let w = if i == 2 { 10.0 } else { 16.0 };
        c.rrect(8.0 * s, (10.0 + i as f32 * 5.0) * s, w * s, 2.4 * s, 1.2 * s, 0x6B5208, 0.85);
    }
    let color = c.into_bitmap();
    unsafe {
        let mask = CreateBitmap(size, size, 1, 1, std::ptr::null());
        let ii = ICONINFO { fIcon: 1, xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color };
        let icon = CreateIconIndirect(&ii);
        DeleteObject(mask);
        DeleteObject(color);
        icon
    }
}
