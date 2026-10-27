// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! The displays connected, and which one the deck is on.
//!
//! A display is known by its monitor's device path from the display configuration
//! (`\\?\DISPLAY#DEL40F4#...`), which stays the same across restarts and rearranging,
//! unlike GDI's `\\.\DISPLAY2`, which Windows renumbers as displays come and go. The
//! configuration's source names are those GDI names, which ties each path to a monitor.
//!
//! Asking for the list goes to the graphics driver, so it's kept until a display, the
//! work area or a scale changes (`forget`, from WM_DISPLAYCHANGE, WM_SETTINGCHANGE and
//! WM_DPICHANGED), and the deck's own display until then or until the choice changes:
//! the deck asks for its place on every layout, which then costs a string comparison.

use std::cell::RefCell;
use std::ptr::null_mut;
use std::rc::Rc;

use windows_sys::Win32::Devices::Display::{
    DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME, DISPLAYCONFIG_DEVICE_INFO_HEADER, DISPLAYCONFIG_MODE_INFO,
    DISPLAYCONFIG_OUTPUT_TECHNOLOGY_DISPLAYPORT_EMBEDDED, DISPLAYCONFIG_OUTPUT_TECHNOLOGY_INTERNAL, DISPLAYCONFIG_OUTPUT_TECHNOLOGY_UDI_EMBEDDED,
    DISPLAYCONFIG_PATH_INFO, DISPLAYCONFIG_SOURCE_DEVICE_NAME, DISPLAYCONFIG_TARGET_DEVICE_NAME, DisplayConfigGetDeviceInfo, GetDisplayConfigBufferSizes,
    QDC_ONLY_ACTIVE_PATHS, QueryDisplayConfig,
};
use windows_sys::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS, LPARAM, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITOR_DEFAULTTOPRIMARY, MONITORINFO, MONITORINFOEXW, MonitorFromPoint};
use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows_sys::Win32::UI::WindowsAndMessaging::MONITORINFOF_PRIMARY;

use crate::win::app;
use crate::win::util::from_wide;

/// A connected display the deck can go on.
pub struct Display {
    /// The monitor's device path: what Settings keeps.
    pub id: String,
    /// Its name as shown: the monitor's own, numbered when two are alike.
    pub name: String,
    pub work: RECT,
    pub s: f32,
    /// Windows' main display.
    pub main: bool,
}

thread_local! {
    static LIST: RefCell<Option<Rc<Vec<Display>>>> = const { RefCell::new(None) };
    /// The deck's display as last worked out: (the choice it was for, work area, scale).
    static DECK: RefCell<Option<(String, RECT, f32)>> = const { RefCell::new(None) };
}

/// Displays, work areas or scales changed: look again when next asked.
pub fn forget() {
    LIST.with(|l| l.borrow_mut().take());
    DECK.with(|d| d.borrow_mut().take());
    // The frame clock's displays too: their refresh rates may have changed.
    crate::win::render::vblank::forget();
}

/// The displays connected, main one first, then left to right.
pub fn list() -> Rc<Vec<Display>> {
    if let Some(l) = LIST.with(|l| l.borrow().clone()) {
        return l;
    }
    let l = Rc::new(enumerate());
    LIST.with(|c| *c.borrow_mut() = Some(l.clone()));
    l
}

/// The deck's display: (work area, DPI scale). The one chosen in Settings while it's
/// connected, else the main display; the choice itself is kept, so the deck goes back
/// to it when it's connected again.
pub fn deck() -> (RECT, f32) {
    let kept = DECK.with(|c| app::store(|s| c.borrow().as_ref().filter(|(id, ..)| *id == s.d.settings.deck_display).map(|&(_, r, k)| (r, k))));
    if let Some(g) = kept {
        return g;
    }
    let chosen = app::store(|s| s.d.settings.deck_display.clone());
    let g = geo_for(&chosen);
    DECK.with(|c| *c.borrow_mut() = Some((chosen, g.0, g.1)));
    g
}

/// Where the deck would be with display `id` chosen: (work area, DPI scale).
pub fn geo_for(id: &str) -> (RECT, f32) {
    if !id.is_empty() {
        let l = list();
        if let Some(i) = index_of(&l, id) {
            return (l[i].work, l[i].s);
        }
    }
    unsafe { info(MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY)) }.map_or((RECT { left: 0, top: 0, right: 1, bottom: 1 }, 1.0), |m| (m.work, m.s))
}

/// Whether display `id` is connected (the main display always is).
pub fn connected(id: &str) -> bool {
    id.is_empty() || index_of(&list(), id).is_some()
}

/// Where display `id` is in `l`.
pub fn index_of(l: &[Display], id: &str) -> Option<usize> {
    find(l.iter().map(|d| d.id.as_str()), id)
}

/// Which of `ids` is `chosen`. Device paths are matched without regard to case, as
/// Windows treats them.
fn find<'a>(ids: impl Iterator<Item = &'a str>, chosen: &str) -> Option<usize> {
    let mut ids = ids;
    ids.position(|i| i.eq_ignore_ascii_case(chosen))
}

/// A short name for a display known only by its device path: the model code in it
/// (`DEL40F4` in `\\?\DISPLAY#DEL40F4#5&...`), or the whole path if it has none.
pub fn short_id(path: &str) -> &str {
    path.split('#').nth(1).filter(|m| !m.is_empty()).unwrap_or(path)
}

/// Names for a list of displays: each one's own (`None` for one without), or `generic`
/// with its place in the list; a name already taken gets " 2", " 3" and so on.
fn names(raw: &[Option<String>], generic: impl Fn(usize) -> String) -> Vec<String> {
    let base: Vec<String> = raw
        .iter()
        .enumerate()
        .map(|(i, n)| n.as_deref().map(str::trim).filter(|n| !n.is_empty()).map_or_else(|| generic(i + 1), str::to_string))
        .collect();
    let mut out: Vec<String> = Vec::with_capacity(base.len());
    for b in &base {
        let mut name = b.clone();
        let mut n = 2;
        // Not one taken already, nor one another display is called by itself.
        while out.contains(&name) || (name != *b && base.contains(&name)) {
            name = format!("{b} {n}");
            n += 1;
        }
        out.push(name);
    }
    out
}

struct Monitor {
    gdi: String,
    rc: RECT,
    work: RECT,
    s: f32,
    primary: bool,
}

unsafe fn info(mon: HMONITOR) -> Option<Monitor> {
    if mon.is_null() {
        return None;
    }
    unsafe {
        let mut mi: MONITORINFOEXW = std::mem::zeroed();
        mi.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
        if GetMonitorInfoW(mon, &mut mi as *mut MONITORINFOEXW as *mut MONITORINFO) == 0 {
            return None;
        }
        let (mut dx, mut dy) = (96u32, 96u32);
        GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        Some(Monitor {
            gdi: from_wide(&mi.szDevice),
            rc: mi.monitorInfo.rcMonitor,
            work: mi.monitorInfo.rcWork,
            s: dx.max(96) as f32 / 96.0,
            primary: mi.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
        })
    }
}

unsafe extern "system" fn each(mon: HMONITOR, _: HDC, _: *mut RECT, lp: LPARAM) -> windows_sys::core::BOOL {
    let out = unsafe { &mut *(lp as *mut Vec<Monitor>) };
    if let Some(m) = unsafe { info(mon) } {
        out.push(m);
    }
    1
}

/// A monitor as the display configuration knows it.
struct Target {
    gdi: String,
    path: String,
    name: Option<String>,
    builtin: bool,
    /// Its refresh rate exactly, as numerator and denominator (99.99 Hz comes as 99990/1000
    /// or the like), when the configuration gives one.
    refresh: Option<(u32, u32)>,
}

/// How long a refresh of the display GDI calls `gdi` (`\\.\DISPLAY1`) lasts, in seconds, from
/// the display configuration's exact rate (99.99 Hz is not 100).
pub fn refresh_period(gdi: &str) -> Option<f64> {
    targets().into_iter().find(|t| t.gdi.eq_ignore_ascii_case(gdi)).and_then(|t| t.refresh).map(|(n, d)| d as f64 / n as f64)
}

/// Every active display path's GDI name, monitor device path and friendly name.
fn targets() -> Vec<Target> {
    let mut out = Vec::new();
    unsafe {
        // A display can come or go between the two calls: then the sizes are asked again.
        let mut paths: Vec<DISPLAYCONFIG_PATH_INFO> = Vec::new();
        for _ in 0..3 {
            let (mut np, mut nm) = (0u32, 0u32);
            if GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut np, &mut nm) != ERROR_SUCCESS {
                return out;
            }
            paths = vec![std::mem::zeroed(); np as usize];
            let mut modes: Vec<DISPLAYCONFIG_MODE_INFO> = vec![std::mem::zeroed(); nm as usize];
            match QueryDisplayConfig(QDC_ONLY_ACTIVE_PATHS, &mut np, paths.as_mut_ptr(), &mut nm, modes.as_mut_ptr(), null_mut()) {
                ERROR_SUCCESS => {
                    paths.truncate(np as usize);
                    break;
                }
                ERROR_INSUFFICIENT_BUFFER => paths.clear(),
                _ => return out,
            }
        }
        for p in &paths {
            let mut src: DISPLAYCONFIG_SOURCE_DEVICE_NAME = std::mem::zeroed();
            src.header = DISPLAYCONFIG_DEVICE_INFO_HEADER {
                r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
                size: size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32,
                adapterId: p.sourceInfo.adapterId,
                id: p.sourceInfo.id,
            };
            let mut tgt: DISPLAYCONFIG_TARGET_DEVICE_NAME = std::mem::zeroed();
            tgt.header = DISPLAYCONFIG_DEVICE_INFO_HEADER {
                r#type: DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
                size: size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32,
                adapterId: p.targetInfo.adapterId,
                id: p.targetInfo.id,
            };
            if DisplayConfigGetDeviceInfo(&mut src.header) != 0 || DisplayConfigGetDeviceInfo(&mut tgt.header) != 0 {
                continue;
            }
            let path = from_wide(&tgt.monitorDevicePath);
            if path.is_empty() {
                continue;
            }
            let name = Some(from_wide(&tgt.monitorFriendlyDeviceName)).filter(|n| !n.trim().is_empty());
            let tech = tgt.outputTechnology;
            let builtin = matches!(
                tech,
                DISPLAYCONFIG_OUTPUT_TECHNOLOGY_INTERNAL | DISPLAYCONFIG_OUTPUT_TECHNOLOGY_DISPLAYPORT_EMBEDDED | DISPLAYCONFIG_OUTPUT_TECHNOLOGY_UDI_EMBEDDED
            );
            let r = p.targetInfo.refreshRate;
            let refresh = (r.Numerator > 0 && r.Denominator > 0).then_some((r.Numerator, r.Denominator));
            out.push(Target { gdi: from_wide(&src.viewGdiDeviceName), path, name, builtin, refresh });
        }
    }
    out
}

fn enumerate() -> Vec<Display> {
    let mut mons: Vec<Monitor> = Vec::new();
    unsafe { EnumDisplayMonitors(null_mut(), std::ptr::null(), Some(each), &mut mons as *mut Vec<Monitor> as LPARAM) };
    mons.sort_by_key(|m| (!m.primary, m.rc.left, m.rc.top));
    let targets = targets();
    // A monitor showing the same picture as another (duplicated) has one GDI name and
    // two paths: the first one stands for it. One the configuration doesn't know (a
    // remote session's) has no lasting name, so it can't be chosen.
    let found: Vec<(&Monitor, &Target)> = mons.iter().filter_map(|m| targets.iter().find(|t| t.gdi.eq_ignore_ascii_case(&m.gdi)).map(|t| (m, t))).collect();
    let raw: Vec<Option<String>> = found.iter().map(|(_, t)| t.name.clone().or_else(|| t.builtin.then(|| t!("display_builtin").to_string()))).collect();
    let shown = names(&raw, |i| t!("display_numbered", i));
    found.into_iter().zip(shown).map(|((m, t), name)| Display { id: t.path.clone(), name, work: m.work, s: m.s, main: m.primary }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(raw: &[Option<&str>]) -> Vec<String> {
        let raw: Vec<Option<String>> = raw.iter().map(|n| n.map(str::to_string)).collect();
        names(&raw, |i| format!("Display {i}"))
    }

    #[test]
    fn displays_keep_their_own_names() {
        assert_eq!(named(&[Some("DELL U2720Q"), Some("LG HDR 4K")]), ["DELL U2720Q", "LG HDR 4K"]);
    }

    #[test]
    fn alike_displays_are_numbered() {
        assert_eq!(named(&[Some("DELL U2720Q"), Some("DELL U2720Q"), Some("DELL U2720Q")]), ["DELL U2720Q", "DELL U2720Q 2", "DELL U2720Q 3"]);
        // A number another display already goes by is skipped.
        assert_eq!(named(&[Some("Dell"), Some("Dell"), Some("Dell 2")]), ["Dell", "Dell 3", "Dell 2"]);
    }

    #[test]
    fn a_display_without_a_name_gets_its_place() {
        assert_eq!(named(&[Some("DELL U2720Q"), None, Some("  ")]), ["DELL U2720Q", "Display 2", "Display 3"]);
        // Even when a display with a name is called that.
        assert_eq!(named(&[Some("Display 2"), None]), ["Display 2", "Display 2 2"]);
    }

    #[test]
    fn the_choice_is_found_by_device_path() {
        let ids = [r"\\?\DISPLAY#DEL40F4#5&1&UID4352#{e6f07b5f}", r"\\?\DISPLAY#GSM5B09#5&1&UID4353#{e6f07b5f}"];
        assert_eq!(find(ids.iter().copied(), r"\\?\display#gsm5b09#5&1&uid4353#{E6F07B5F}"), Some(1));
        assert_eq!(find(ids.iter().copied(), r"\\?\DISPLAY#AUO1E8C#4&2&UID8388688#{e6f07b5f}"), None);
        assert_eq!(find(std::iter::empty(), "x"), None);
    }

    #[test]
    fn a_missing_display_is_known_by_its_model() {
        assert_eq!(short_id(r"\\?\DISPLAY#DEL40F4#5&2b6d&0&UID4352#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}"), "DEL40F4");
        assert_eq!(short_id("odd"), "odd");
    }
}
