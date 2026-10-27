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
    CreateIconIndirect, GetClientRect, HICON, ICONINFO, UPDATELAYEREDWINDOWINFO, ULW_ALPHA, UpdateLayeredWindow, UpdateLayeredWindowIndirect,
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
    /// Take the canvas's size of pixels from `hdc` at (`x`, `y`), opaque.
    pub fn grab(&mut self, hdc: HDC, x: i32, y: i32) {
        unsafe { BitBlt(self.img.dc, 0, 0, self.w, self.h, hdc, x, y, SRCCOPY) };
        self.make_opaque_rect(RECT { left: 0, top: 0, right: self.w, bottom: self.h });
    }

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

    /// `present`, where only `dirty` (canvas coordinates) changed since the window was last
    /// given the canvas at the same place and size: the compositor takes just that.
    pub fn present_dirty(&self, hwnd: HWND, x: i32, y: i32, dirty: RECT) {
        unsafe {
            GdiFlush();
            let dst = POINT { x, y };
            let size = SIZE { cx: self.w, cy: self.h };
            let src = POINT { x: 0, y: 0 };
            let bf = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
            let info = UPDATELAYEREDWINDOWINFO {
                cbSize: size_of::<UPDATELAYEREDWINDOWINFO>() as u32,
                hdcDst: null_mut(),
                pptDst: &dst,
                psize: &size,
                hdcSrc: self.img.dc,
                pptSrc: &src,
                crKey: 0,
                pblend: &bf,
                dwFlags: ULW_ALPHA,
                prcDirty: &dirty,
            };
            if UpdateLayeredWindowIndirect(hwnd, &info) == 0 {
                self.present(hwnd, x, y);
            }
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
        let k = (alpha.min(1.0) * 256.0) as u32;
        let px = self.px();
        for py in y0..y1 {
            let (row, src_row) = ((py * cw) as usize, ((py - oy) * img.w) as usize);
            // Rows clear of the rounded corners are whole across, but for a pixel at each
            // end: no edge to work out there, and an opaque picture at full strength is
            // just copied.
            let (mut a, mut b) = (x0, x0);
            if ((py as f32 + 0.5) - cy).abs() < hy - r - 1.0 {
                a = ((clip.x + 1.0).ceil() as i32).clamp(x0, x1);
                b = ((clip.x + clip.w - 1.0).floor() as i32).clamp(a, x1);
                for pxx in a..b {
                    let (s, dst) = (img.px[src_row + (pxx - ox) as usize], px[row + pxx as usize]);
                    if k >= 256 && s >> 24 == 0xFF {
                        px[row + pxx as usize] = s;
                        continue;
                    }
                    let keep = 256 - k.min(256) * (s >> 24) / 255;
                    let ch = |sh: u32| ((((s >> sh) & 0xFF) * k.min(256) + ((dst >> sh) & 0xFF) * keep) >> 8) << sh;
                    px[row + pxx as usize] = ch(24) | ch(16) | ch(8) | ch(0);
                }
            }
            for pxx in (x0..a).chain(b..x1) {
                let (sx, sy) = (pxx as f32 + 0.5, py as f32 + 0.5);
                let qx = (sx - cx).abs() - (hx - r);
                let qy = (sy - cy).abs() - (hy - r);
                let d = if qx > 0.0 && qy > 0.0 { (qx * qx + qy * qy).sqrt() - r } else { qx.max(qy) - r };
                let a = alpha * (0.5 - d).clamp(0.0, 1.0);
                if a <= 0.0 {
                    continue;
                }
                // Both premultiplied: the picture over what's there, by its own opacity
                // too (a window's picture is opaque; a PNG may not be).
                let (s, dst) = (img.px[src_row + (pxx - ox) as usize], px[row + pxx as usize]);
                let k = (a * 256.0) as u32;
                let keep = 256 - k * (s >> 24) / 255;
                let ch = |sh: u32| ((((s >> sh) & 0xFF) * k + ((dst >> sh) & 0xFF) * keep) >> 8) << sh;
                px[row + pxx as usize] = ch(24) | ch(16) | ch(8) | ch(0);
            }
        }
    }
}

impl Canvas {
    /// `img` over the canvas at `alpha`, turned `angle` radians (clockwise) about its point
    /// `from`, which lands at `to`; nothing outside `clip`. Turned, it's sampled between
    /// pixels (bilinear); unturned, it goes on whole pixels as it is, so text stays crisp.
    /// For text drawn once and moved about (a card's label as the card moves and leans):
    /// a few thousand pixels, where drawing the text again costs a Direct2D pass.
    #[allow(clippy::too_many_arguments)]
    pub fn over_turned(&mut self, img: &Image, from: (f32, f32), to: (f32, f32), angle: f32, clip: RECT, alpha: f32) {
        if alpha <= 0.002 || img.w <= 0 || img.h <= 0 {
            return;
        }
        unsafe { GdiFlush() };
        let k = (alpha.min(1.0) * 255.0).round() as u32;
        let (cw, chh) = (self.w, self.h);
        let (iw, ih) = (img.w, img.h);
        let put = |px: &mut [u32], i: usize, s: u32| {
            let s = if k < 255 {
                let f = |sh: u32| ((((s >> sh) & 0xFF) * k + 127) / 255) << sh;
                f(24) | f(16) | f(8) | f(0)
            } else {
                s
            };
            let sa = s >> 24;
            if sa == 0 {
                return;
            }
            let d = px[i];
            let inv = 255 - sa;
            let c = |sh: u32| -> u32 { (((s >> sh) & 0xFF) + (((d >> sh) & 0xFF) * inv + 127) / 255).min(255) << sh };
            px[i] = c(24) | c(16) | c(8) | c(0);
        };
        if angle.abs() < 1e-4 {
            let (dx, dy) = ((to.0 - from.0).round() as i32, (to.1 - from.1).round() as i32);
            let (x0, y0) = (dx.max(clip.left).max(0), dy.max(clip.top).max(0));
            let (x1, y1) = ((dx + iw).min(clip.right).min(cw), (dy + ih).min(clip.bottom).min(chh));
            let px = self.px();
            for y in y0..y1 {
                let (row, src) = ((y * cw) as usize, ((y - dy) * iw) as usize);
                for x in x0..x1 {
                    put(px, row + x as usize, img.px[src + (x - dx) as usize]);
                }
            }
            return;
        }
        let (sn, cs) = angle.sin_cos();
        // Where the picture's corners land, for the rows and columns to visit.
        let (mut bx0, mut by0, mut bx1, mut by1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for (x, y) in [(0.0, 0.0), (iw as f32, 0.0), (0.0, ih as f32), (iw as f32, ih as f32)] {
            let (dx, dy) = (x - from.0, y - from.1);
            let (fx, fy) = (to.0 + dx * cs - dy * sn, to.1 + dx * sn + dy * cs);
            bx0 = bx0.min(fx);
            by0 = by0.min(fy);
            bx1 = bx1.max(fx);
            by1 = by1.max(fy);
        }
        let x0 = (bx0.floor() as i32).max(clip.left).max(0);
        let y0 = (by0.floor() as i32).max(clip.top).max(0);
        let x1 = (bx1.ceil() as i32 + 1).min(clip.right).min(cw);
        let y1 = (by1.ceil() as i32 + 1).min(clip.bottom).min(chh);
        let at = |x: i32, y: i32| -> u32 { if x < 0 || y < 0 || x >= iw || y >= ih { 0 } else { img.px[(y * iw + x) as usize] } };
        let px = self.px();
        for y in y0..y1 {
            let row = (y * cw) as usize;
            for x in x0..x1 {
                // Back from the canvas into the picture: the pixel's centre, turned back.
                let (dx, dy) = (x as f32 + 0.5 - to.0, y as f32 + 0.5 - to.1);
                let (sx, sy) = (from.0 + dx * cs + dy * sn - 0.5, from.1 - dx * sn + dy * cs - 0.5);
                let (fx, fy) = (sx.floor(), sy.floor());
                let (ix, iy) = (fx as i32, fy as i32);
                if ix < -1 || iy < -1 || ix >= iw || iy >= ih {
                    continue;
                }
                let (wx, wy) = (((sx - fx) * 256.0) as u32, ((sy - fy) * 256.0) as u32);
                let (p00, p10, p01, p11) = (at(ix, iy), at(ix + 1, iy), at(ix, iy + 1), at(ix + 1, iy + 1));
                if (p00 | p10 | p01 | p11) == 0 {
                    continue;
                }
                let mix = |sh: u32| -> u32 {
                    let c = |p: u32| (p >> sh) & 0xFF;
                    let top = c(p00) * (256 - wx) + c(p10) * wx;
                    let bottom = c(p01) * (256 - wx) + c(p11) * wx;
                    ((top * (256 - wy) + bottom * wy + (1 << 15)) >> 16).min(255) << sh
                };
                put(px, row + x as usize, mix(24) | mix(16) | mix(8) | mix(0));
            }
        }
    }

    /// `img` over the canvas with its top-left at (`x`, `y`), as text is put down (both
    /// premultiplied): text drawn on a clear canvas and kept as a picture comes out exactly
    /// as if drawn here.
    pub fn over(&mut self, img: &Image, x: i32, y: i32) {
        unsafe { GdiFlush() };
        let (cw, ch) = (self.w, self.h);
        let px = self.px();
        for iy in 0.max(-y)..img.h.min(ch - y) {
            let (row, src_row) = (((iy + y) * cw) as usize, (iy * img.w) as usize);
            for ix in 0.max(-x)..img.w.min(cw - x) {
                let s = img.px[src_row + ix as usize];
                let sa = s >> 24;
                if sa == 0 {
                    continue;
                }
                let i = row + (ix + x) as usize;
                let d = px[i];
                let inv = 255 - sa;
                let c = |sh: u32| -> u32 { (((s >> sh) & 0xFF) + (((d >> sh) & 0xFF) * inv + 127) / 255).min(255) << sh };
                px[i] = c(24) | c(16) | c(8) | c(0);
            }
        }
    }
}

/// A picture: of a window, for animating it as it opens or closes, or of a note's
/// attachment, for its preview.
pub struct Image {
    pub w: i32,
    pub h: i32,
    px: Vec<u32>,
}

impl Image {
    /// From `w` x `h` premultiplied BGRA pixels.
    pub fn from_pixels(w: i32, h: i32, px: Vec<u32>) -> Image {
        Image { w, h, px }
    }
}

#[link(name = "user32")]
unsafe extern "system" {
    fn PrintWindow(hwnd: HWND, hdc: HDC, flags: u32) -> i32;
}

/// What `hwnd` (children and all) looks like right now, as Windows composes it: painted
/// first if anything is pending. None if Windows can't provide it.
pub fn capture(hwnd: HWND) -> Option<Image> {
    grab(hwnd, 2)
}

/// `capture` without waiting: None too while the compositor hasn't had a window just made
/// (it comes back black), to try again a frame later.
pub fn capture_now(hwnd: HWND) -> Option<Image> {
    grab(hwnd, 1)
}

fn grab(hwnd: HWND, tries: u32) -> Option<Image> {
    const PW_CLIENTONLY: u32 = 1;
    const PW_RENDERFULLCONTENT: u32 = 2;
    unsafe {
        let mut rc: RECT = std::mem::zeroed();
        GetClientRect(hwnd, &mut rc);
        let (w, h) = (rc.right, rc.bottom);
        if w <= 0 || h <= 0 {
            return None;
        }
        // Only what's left to paint: a window on screen is up to date already, and painting
        // it all again (its text, then the overlays over it) cost a closing note tens of ms
        // before its card could set off, and could show a frame of unstyled text first. A
        // window just made has all of it left to paint.
        RedrawWindow(hwnd, std::ptr::null(), null_mut(), RDW_UPDATENOW | RDW_ALLCHILDREN);
        let dib = Dib::new(w, h);
        // A window just made can come back black, the compositor not having had it yet (a
        // new note's card went yellow, brown, black on its way open): give it a frame, and
        // if it's still black, there's no picture.
        for tried in 0..tries {
            if tried > 0 {
                windows_sys::Win32::Graphics::Dwm::DwmFlush();
            }
            if PrintWindow(hwnd, dib.dc, PW_CLIENTONLY | PW_RENDERFULLCONTENT) == 0 {
                return None;
            }
            GdiFlush();
            let px = std::slice::from_raw_parts(dib.px, (w * h) as usize);
            if !blank(px) {
                return Some(Image { w, h, px: px.iter().map(|p| p | 0xFF00_0000).collect() });
            }
        }
        None
    }
}

/// Whether a window's picture is a quarter or more pure black: what Windows gives where it
/// hasn't had the window drawn yet. A note is paper, never black (nor is its text).
fn blank(px: &[u32]) -> bool {
    let seen = px.iter().step_by(7).count().max(1);
    px.iter().step_by(7).filter(|p| *p & 0x00FF_FFFF == 0).count() * 4 >= seen
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A picture clipped to a rounded card comes out the same whether a pixel is worked out
    /// on its own or, away from the corners, as part of a row.
    #[test]
    fn pictures_on_cards_are_drawn_as_worked_out() {
        let (w, h) = (40, 30);
        let src: Vec<u32> = (0..w * h).map(|i| if i % 3 == 0 { 0x80402010 } else { 0xFF00_0000 | (i as u32).wrapping_mul(2_654_435_761) >> 8 }).collect();
        let img = Image::from_pixels(w, h, src.clone());
        for alpha in [1.0f32, 0.6] {
            let clip = Shape::new(5.3, 4.6, 36.0, 26.0, 6.0);
            let mut c = Canvas::new(60, 50);
            c.clear(0xFF33_6699);
            c.image(&img, 3.0, 2.0, &clip, alpha);
            let got = c.copy_out(RECT { left: 0, top: 0, right: 60, bottom: 50 });
            // Every pixel by itself, as before rows were taken whole.
            let (hx, hy) = (clip.w / 2.0, clip.h / 2.0);
            let (cx, cy, r) = (clip.x + hx, clip.y + hy, clip.r);
            for y in 0..50 {
                for x in 0..60 {
                    let (sx, sy) = (x - 3, y - 2);
                    let mut want = 0xFF33_6699u32;
                    if (0..w).contains(&sx) && (0..h).contains(&sy) {
                        let (qx, qy) = ((x as f32 + 0.5 - cx).abs() - (hx - r), (y as f32 + 0.5 - cy).abs() - (hy - r));
                        let d = if qx > 0.0 && qy > 0.0 { (qx * qx + qy * qy).sqrt() - r } else { qx.max(qy) - r };
                        let a = alpha * (0.5 - d).clamp(0.0, 1.0);
                        if a > 0.0 {
                            let s = src[(sy * w + sx) as usize];
                            let k = (a * 256.0) as u32;
                            let keep = 256 - k * (s >> 24) / 255;
                            let ch = |sh: u32| ((((s >> sh) & 0xFF) * k + ((want >> sh) & 0xFF) * keep) >> 8) << sh;
                            want = ch(24) | ch(16) | ch(8) | ch(0);
                        }
                    }
                    assert_eq!(got[(y * 60 + x) as usize], want, "({x}, {y}) at {alpha}");
                }
            }
        }
    }

    /// A window Windows hasn't drawn yet comes back black, and isn't used as its picture; a
    /// note, paper with text and a dark picture in it, is.
    #[test]
    fn black_captures_are_left_out() {
        let mut note = vec![0xFFF6_E7A6u32; 400 * 300];
        for p in note.iter_mut().skip(4000).take(20_000) {
            *p = 0xFF00_0000; // a dark picture, a sixth of the note
        }
        for p in note.iter_mut().step_by(5) {
            *p = 0xFF1D_1D1F; // text
        }
        assert!(!blank(&note));
        assert!(blank(&vec![0xFF00_0000u32; 400 * 300]));
        // Painted only at the top (a header over a black body).
        let mut half = vec![0xFF00_0000u32; 400 * 300];
        half[..400 * 40].fill(0xFFF6_E7A6);
        assert!(blank(&half));
    }

    /// A picture put down unturned lands on whole pixels exactly as it is; turned, it lands
    /// where it's turned to, and fades as asked.
    #[test]
    fn pictures_put_down_turned() {
        let mut px = vec![0u32; 6 * 10];
        for y in 2..8 {
            px[y * 6 + 2] = 0xFF20_4060;
            px[y * 6 + 3] = 0x8010_2030;
        }
        let img = Image::from_pixels(6, 10, px);
        let all = RECT { left: 0, top: 0, right: 40, bottom: 40 };
        let mut a = Canvas::new(40, 40);
        a.clear(0xFFFF_FFFF);
        a.over(&img, 10, 12);
        let mut b = Canvas::new(40, 40);
        b.clear(0xFFFF_FFFF);
        b.over_turned(&img, (1.0, 1.0), (11.2, 12.8), 0.0, all, 1.0);
        assert_eq!(a.copy_out(all), b.copy_out(all));
        // A quarter turn clockwise about the picture's top-left: its column 2 becomes the
        // canvas's row 2 below the point it lands on, going left.
        let mut c = Canvas::new(40, 40);
        c.clear(0);
        c.over_turned(&img, (0.0, 0.0), (20.0, 20.0), std::f32::consts::FRAC_PI_2, all, 1.0);
        let at = |c: &mut Canvas, x: i32, y: i32| c.copy_out(RECT { left: x, top: y, right: x + 1, bottom: y + 1 })[0];
        assert_eq!(at(&mut c, 20 - 5, 22) >> 24, 0xFF);
        assert_eq!(at(&mut c, 20 - 1, 22), 0);
        // Half faded, half the ink.
        let mut d = Canvas::new(40, 40);
        d.clear(0);
        d.over_turned(&img, (0.0, 0.0), (10.0, 10.0), 0.0, all, 0.5);
        assert_eq!(at(&mut d, 12, 12) >> 24, 0x80);
    }
}
