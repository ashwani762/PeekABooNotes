// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! DirectWrite text, the same engine WinUI and Edge use: proper gamma, optical sizes of
//! Segoe UI Variable, real rotation. Text is rendered by a Direct2D DC render target into a
//! cleared scratch region bound to exactly the text's bounds, then composited with
//! premultiplied alpha — correct on transparent layered windows and opaque windows alike.
//! Greyscale anti-aliasing throughout (no colour fringes on any background).

use std::cell::RefCell;

use windows::Win32::Foundation::RECT as WRECT;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::HDC as WHDC;
use windows::core::PCWSTR;
use windows_numerics::{Matrix3x2, Vector2};
use windows_sys::Win32::Foundation::RECT;
use windows_sys::Win32::Graphics::Gdi::{DT_BOTTOM, DT_CENTER, DT_END_ELLIPSIS, DT_RIGHT, DT_VCENTER, DT_WORDBREAK, HDC};

struct Engine {
    dwf: IDWriteFactory,
    rt: ID2D1DCRenderTarget,
    brush: ID2D1SolidColorBrush,
}

/// Keyed by the text format's address, so each entry also holds that format: a format
/// freed and re-allocated at the same address must never hit another font's layouts.
type LayoutKey = (String, usize, u32, u32, u32, bool, bool);
type Cached = (IDWriteTextFormat, IDWriteTextLayout);

thread_local! {
    static ENGINE: RefCell<Option<Engine>> = const { RefCell::new(None) };
    static LAYOUTS: std::cell::RefCell<std::collections::HashMap<LayoutKey, Cached>> = std::cell::RefCell::new(std::collections::HashMap::new());
}

fn engine<R>(f: impl FnOnce(&Engine) -> R) -> Option<R> {
    ENGINE.with(|e| {
        if e.borrow().is_none() {
            let made = make_engine();
            *e.borrow_mut() = made;
        }
        e.borrow().as_ref().map(f)
    })
}

/// Let go of everything text rendering holds beyond the fonts themselves: cached layouts
/// and the render target (whose surface stays as large as the largest text ever drawn).
/// Both come back on the next draw.
pub fn trim() {
    LAYOUTS.with(|c| c.borrow_mut().clear());
    ENGINE.with(|e| {
        if let Ok(mut e) = e.try_borrow_mut() {
            e.take();
        }
    });
}

fn make_engine() -> Option<Engine> {
    {
        unsafe {
            let d2d: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None).ok()?;
            let dwf: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).ok()?;
            let props = D2D1_RENDER_TARGET_PROPERTIES {
                // Software is faster here than the GPU: no readback for small text regions.
                r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
                dpiX: 96.0,
                dpiY: 96.0,
                usage: D2D1_RENDER_TARGET_USAGE_GDI_COMPATIBLE,
                minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
            };
            let rt = d2d.CreateDCRenderTarget(&props).ok()?;
            rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            let brush = rt.CreateSolidColorBrush(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 1.0 }, None).ok()?;
            Some(Engine { dwf, rt, brush })
        }
    }
}

/// A text style: family + weight + size (+ optional italic / underline / strike).
/// Cloning is cheap (COM reference counts).
#[derive(Clone)]
pub struct Font {
    fmt: Option<IDWriteTextFormat>,
    trim: Option<IDWriteInlineObject>,
    /// Distance from the top of the line box to the baseline, in px.
    pub ascent: i32,
    pub descent: i32,
    /// Capital-letter height (for optically centring icons against text).
    pub cap: i32,
    underline: bool,
    strike: bool,
}

impl Font {
    pub fn new(px: f32, weight: u32, face: &str) -> Font {
        Font::new_styled(px, weight, face, false, false, false)
    }

    pub fn new_styled(px: f32, weight: u32, face: &str, italic: bool, underline: bool, strike: bool) -> Font {
        let size = px.max(1.0);
        let name = crate::win::util::wide(face);
        let (fmt, trim) = engine(|e| unsafe {
            let style = if italic { DWRITE_FONT_STYLE_ITALIC } else { DWRITE_FONT_STYLE_NORMAL };
            let fmt = e
                .dwf
                .CreateTextFormat(PCWSTR(name.as_ptr()), None, DWRITE_FONT_WEIGHT(weight as i32), style, DWRITE_FONT_STRETCH_NORMAL, size, PCWSTR(crate::common::i18n::tag_wide().as_ptr()))
                .ok();
            let trim = fmt.as_ref().and_then(|f| e.dwf.CreateEllipsisTrimmingSign(f).ok());
            (fmt, trim)
        })
        .unwrap_or((None, None));
        let mut f = Font { fmt, trim, ascent: (size * 0.93) as i32, descent: (size * 0.25) as i32, cap: (size * 0.7).round() as i32, underline, strike };
        if let Some(l) = f.layout("Hg", 10_000.0, 10_000.0, 0) {
            let mut lines = [DWRITE_LINE_METRICS::default(); 1];
            let mut n = 0u32;
            if unsafe { l.GetLineMetrics(Some(&mut lines), &mut n) }.is_ok() && n > 0 {
                f.ascent = lines[0].baseline.round() as i32;
                f.descent = (lines[0].height - lines[0].baseline).round() as i32;
            }
        }
        f
    }

    /// A layout for `s` in a `w`×`h` box; `flags` are DT_* alignment / wrap / ellipsis bits.
    /// Layouts are immutable once built, so identical requests (most text, most frames)
    /// reuse a cached one.
    fn layout(&self, s: &str, w: f32, h: f32, flags: u32) -> Option<IDWriteTextLayout> {
        let fmt = self.fmt.as_ref()?;
        let key = (s.to_string(), windows::core::Interface::as_raw(fmt) as usize, w.to_bits(), h.to_bits(), flags, self.underline, self.strike);
        if let Some(l) = LAYOUTS.with(|c| c.borrow().get(&key).map(|(_, l)| l.clone())) {
            return Some(l);
        }
        let l = self.build_layout(s, w, h, flags)?;
        LAYOUTS.with(|c| {
            let mut c = c.borrow_mut();
            if c.len() > 1500 {
                c.clear();
            }
            c.insert(key, (fmt.clone(), l.clone()));
        });
        Some(l)
    }

    fn build_layout(&self, s: &str, w: f32, h: f32, flags: u32) -> Option<IDWriteTextLayout> {
        let fmt = self.fmt.as_ref()?;
        let text: Vec<u16> = s.encode_utf16().collect();
        engine(|e| unsafe {
            let l = e.dwf.CreateTextLayout(&text, fmt, w.max(1.0), h.max(1.0)).ok()?;
            let align = if flags & DT_CENTER != 0 {
                DWRITE_TEXT_ALIGNMENT_CENTER
            } else if flags & DT_RIGHT != 0 {
                DWRITE_TEXT_ALIGNMENT_TRAILING
            } else {
                DWRITE_TEXT_ALIGNMENT_LEADING
            };
            let _ = l.SetTextAlignment(align);
            let para = if flags & DT_VCENTER != 0 {
                DWRITE_PARAGRAPH_ALIGNMENT_CENTER
            } else if flags & DT_BOTTOM != 0 {
                DWRITE_PARAGRAPH_ALIGNMENT_FAR
            } else {
                DWRITE_PARAGRAPH_ALIGNMENT_NEAR
            };
            let _ = l.SetParagraphAlignment(para);
            let _ = l.SetWordWrapping(if flags & DT_WORDBREAK != 0 { DWRITE_WORD_WRAPPING_WRAP } else { DWRITE_WORD_WRAPPING_NO_WRAP });
            if flags & DT_END_ELLIPSIS != 0 {
                let t = DWRITE_TRIMMING { granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER, delimiter: 0, delimiterCount: 0 };
                let _ = l.SetTrimming(&t, self.trim.as_ref());
            }
            let all = DWRITE_TEXT_RANGE { startPosition: 0, length: text.len() as u32 };
            if self.underline {
                let _ = l.SetUnderline(true, all);
            }
            if self.strike {
                let _ = l.SetStrikethrough(true, all);
            }
            Some(l)
        })
        .flatten()
    }

    /// The same font with a strikethrough.
    pub fn struck(&self) -> Font {
        Font { strike: true, ..self.clone() }
    }

    /// Height of `s` word-wrapped to `w` px.
    pub fn wrapped_height(&self, s: &str, w: i32) -> i32 {
        if s.is_empty() {
            return self.ascent + self.descent;
        }
        let Some(l) = self.layout(s, w.max(1) as f32, 100_000.0, DT_WORDBREAK) else { return 0 };
        let mut m = DWRITE_TEXT_METRICS::default();
        if unsafe { l.GetMetrics(&mut m) }.is_err() {
            return 0;
        }
        m.height.ceil() as i32
    }

    /// Advance width and height of a single line of `s`.
    pub fn measure(&self, s: &str) -> (i32, i32) {
        if s.is_empty() {
            return (0, self.ascent + self.descent);
        }
        let Some(l) = self.layout(s, 100_000.0, 10_000.0, 0) else { return (0, 0) };
        let mut m = DWRITE_TEXT_METRICS::default();
        if unsafe { l.GetMetrics(&mut m) }.is_err() {
            return (0, 0);
        }
        (m.widthIncludingTrailingWhitespace.ceil() as i32, m.height.ceil() as i32)
    }

    /// `s` shortened with an ellipsis to fit `max` px.
    pub fn fit(&self, s: &str, max: i32) -> String {
        if self.measure(s).0 <= max {
            return s.to_string();
        }
        let chars: Vec<char> = s.chars().collect();
        let (mut lo, mut hi) = (0usize, chars.len());
        while lo < hi {
            let mid = (lo + hi).div_ceil(2);
            let t: String = chars[..mid].iter().collect::<String>() + "\u{2026}";
            if self.measure(&t).0 <= max { lo = mid } else { hi = mid - 1 }
        }
        chars[..lo].iter().collect::<String>().trim_end().to_string() + "\u{2026}"
    }

    /// A path shortened in the middle to fit `max` px, keeping its root and its end
    /// (the folder that matters): `C:\Users\…\PeekABoo Notes`.
    pub fn fit_path(&self, s: &str, max: i32) -> String {
        if self.measure(s).0 <= max {
            return s.to_string();
        }
        let chars: Vec<char> = s.chars().collect();
        let head: String = chars.iter().take(3).collect();
        let tail = |n: usize| chars[chars.len() - n..].iter().collect::<String>();
        let (mut lo, mut hi) = (0usize, chars.len().saturating_sub(3));
        while lo < hi {
            let mid = (lo + hi).div_ceil(2);
            if self.measure(&format!("{head}\u{2026}{}", tail(mid))).0 <= max { lo = mid } else { hi = mid - 1 }
        }
        format!("{head}\u{2026}{}", tail(lo))
    }
}

/// One recorded text draw (see `Target::batch`).
pub struct Op {
    layout: IDWriteTextLayout,
    origin: (f32, f32),
    angle: f32,
    pivot: (f32, f32),
    bounds: RECT,
    rgb: u32,
    alpha: f32,
}

/// Rendering surface hooks provided by the canvas.
pub struct Target<'a> {
    pub scratch_dc: HDC,
    pub scratch: &'a mut [u32],
    pub img: &'a mut [u32],
    pub w: i32,
    pub h: i32,
    /// When set, text is recorded and drawn later in one Direct2D pass (`flush`).
    pub batch: Option<&'a mut Vec<Op>>,
}

/// Queue (or immediately draw) a layout at `origin` (box top-left, canvas px), optionally
/// rotated by `angle` degrees clockwise around `pivot`, visible only inside `bounds`.
#[allow(clippy::too_many_arguments)]
fn draw(t: &mut Target, layout: IDWriteTextLayout, origin: (f32, f32), angle: f32, pivot: (f32, f32), bounds: RECT, rgb: u32, alpha: f32) {
    let b = RECT { left: bounds.left.max(0), top: bounds.top.max(0), right: bounds.right.min(t.w), bottom: bounds.bottom.min(t.h) };
    if b.right <= b.left || b.bottom <= b.top || alpha <= 0.003 {
        return;
    }
    let op = Op { layout, origin, angle, pivot, bounds: b, rgb, alpha };
    match t.batch.as_mut() {
        Some(q) => q.push(op),
        None => render(t, &[op]),
    }
}

/// Draw recorded ops: one BindDC/BeginDraw/EndDraw over their union, then a single
/// premultiplied composite of that region onto the image.
pub fn render(t: &mut Target, ops: &[Op]) {
    if ops.is_empty() {
        return;
    }
    let mut u = ops[0].bounds;
    for o in &ops[1..] {
        u.left = u.left.min(o.bounds.left);
        u.top = u.top.min(o.bounds.top);
        u.right = u.right.max(o.bounds.right);
        u.bottom = u.bottom.max(o.bounds.bottom);
    }
    let ok = engine(|e| unsafe {
        let sub = WRECT { left: u.left, top: u.top, right: u.right, bottom: u.bottom };
        if e.rt.BindDC(WHDC(t.scratch_dc as _), &sub).is_err() {
            return false;
        }
        e.rt.BeginDraw();
        e.rt.Clear(Some(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }));
        let shift = Matrix3x2::translation(-(u.left as f32), -(u.top as f32));
        for o in ops {
            e.rt.SetTransform(&shift);
            let clip = D2D_RECT_F { left: o.bounds.left as f32, top: o.bounds.top as f32, right: o.bounds.right as f32, bottom: o.bounds.bottom as f32 };
            e.rt.PushAxisAlignedClip(&clip, D2D1_ANTIALIAS_MODE_ALIASED);
            let m = if o.angle.abs() > 0.01 { Matrix3x2::rotation_around(o.angle, Vector2 { X: o.pivot.0, Y: o.pivot.1 }) * shift } else { shift };
            e.rt.SetTransform(&m);
            let c = |sh: u32| ((o.rgb >> sh) & 0xFF) as f32 / 255.0;
            e.brush.SetColor(&D2D1_COLOR_F { r: c(16), g: c(8), b: c(0), a: o.alpha.clamp(0.0, 1.0) });
            e.rt.DrawTextLayout(Vector2 { X: o.origin.0, Y: o.origin.1 }, &o.layout, &e.brush, D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT);
            e.rt.SetTransform(&shift);
            e.rt.PopAxisAlignedClip();
        }
        e.rt.EndDraw(None, None).is_ok()
    })
    .unwrap_or(false);
    if !ok {
        return;
    }
    unsafe { windows_sys::Win32::Graphics::Gdi::GdiFlush() };
    // Premultiplied "over" of the rendered text onto the image.
    for y in u.top..u.bottom {
        let row = (y * t.w) as usize;
        for x in u.left..u.right {
            let i = row + x as usize;
            let s = t.scratch[i];
            let sa = s >> 24;
            if sa == 0 {
                continue;
            }
            let d = t.img[i];
            let inv = 255 - sa;
            let ch = |sh: u32| -> u32 {
                let sc = (s >> sh) & 0xFF;
                let dc = (((d >> sh) & 0xFF) * inv + 127) / 255;
                (sc + dc).min(255) << sh
            };
            t.img[i] = ch(24) | ch(16) | ch(8) | ch(0);
        }
    }
}

/// Text in a box. `flags`: DT_LEFT/CENTER/RIGHT, DT_VCENTER/BOTTOM, DT_WORDBREAK, DT_END_ELLIPSIS.
#[allow(clippy::too_many_arguments)]
pub fn text(t: &mut Target, s: &str, f: &Font, r: RECT, clip: Option<RECT>, rgb: u32, alpha: f32, flags: u32) {
    if s.is_empty() {
        return;
    }
    let Some(l) = f.layout(s, (r.right - r.left) as f32, (r.bottom - r.top) as f32, flags) else { return };
    let c = clip.unwrap_or(r);
    let b = RECT { left: r.left.max(c.left), top: r.top.max(c.top), right: r.right.min(c.right), bottom: r.bottom.min(c.bottom) };
    // Let glyph overhang (italics, descenders) breathe a little beyond the box.
    let b = if clip.is_none() { RECT { left: b.left - 2, top: b.top - 2, right: b.right + 2, bottom: b.bottom + 2 } } else { b };
    draw(t, l, (r.left as f32, r.top as f32), 0.0, (0.0, 0.0), b, rgb, alpha);
}

/// `text` with the usual overhang slack around `r`, but nothing drawn below `bottom`
/// (a cut-off line must not show the tops of the next one).
#[allow(clippy::too_many_arguments)]
pub fn text_above(t: &mut Target, s: &str, f: &Font, r: RECT, bottom: i32, rgb: u32, alpha: f32, flags: u32) {
    if s.is_empty() {
        return;
    }
    let Some(l) = f.layout(s, (r.right - r.left) as f32, (r.bottom - r.top) as f32, flags) else { return };
    let b = RECT { left: r.left - 2, top: r.top - 2, right: r.right + 2, bottom: (r.bottom + 2).min(bottom) };
    draw(t, l, (r.left as f32, r.top as f32), 0.0, (0.0, 0.0), b, rgb, alpha);
}

/// One line whose baseline sits at `baseline`; returns its advance width.
#[allow(clippy::too_many_arguments)]
pub fn line(t: &mut Target, s: &str, f: &Font, x: i32, baseline: i32, w: i32, clip: Option<RECT>, rgb: u32, alpha: f32, align: u32) -> i32 {
    if s.is_empty() {
        return 0;
    }
    let top = baseline - f.ascent;
    let h = f.ascent + f.descent;
    let Some(l) = f.layout(s, w as f32, h as f32, align | DT_END_ELLIPSIS) else { return 0 };
    let r = RECT { left: x, top, right: x + w, bottom: top + h };
    // Glyphs overshoot the font's nominal box (descenders of p/g/y, accents), so blend
    // a little beyond it rather than slicing them flat.
    let slack = (h / 4).max(3);
    let c = clip.unwrap_or(RECT { left: x - 2, top: top - slack, right: x + w + 2, bottom: top + h + slack });
    let b = RECT { left: r.left.max(c.left) - 1, top: (r.top - slack).max(c.top), right: r.right.min(c.right) + 1, bottom: (r.bottom + slack).min(c.bottom) };
    let mut m = DWRITE_TEXT_METRICS::default();
    let _ = unsafe { l.GetMetrics(&mut m) };
    draw(t, l, (x as f32, top as f32), 0.0, (0.0, 0.0), b, rgb, alpha);
    m.widthIncludingTrailingWhitespace.ceil() as i32
}

/// A single line laid out at `origin` (box top-left) and rotated `angle` degrees
/// (clockwise) around `pivot`, blended inside `clip`.
#[allow(clippy::too_many_arguments)]
pub fn rotated(t: &mut Target, s: &str, f: &Font, origin: (f32, f32), angle: f32, pivot: (f32, f32), clip: RECT, rgb: u32, alpha: f32) {
    if s.is_empty() {
        return;
    }
    let Some(l) = f.layout(s, 10_000.0, (f.ascent + f.descent) as f32, 0) else { return };
    draw(t, l, origin, angle, pivot, clip, rgb, alpha);
}
