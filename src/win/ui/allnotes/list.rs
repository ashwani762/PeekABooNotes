// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! The note list: rows, ordering, selection, scrolling and drag-to-reorder.

use super::*;
use crate::common::store::Note;

pub(super) fn set_mode(l: &mut Lib, mode: usize) {
    if l.mode != mode {
        if (l.mode == 1) != (mode == 1) {
            unsafe { SetWindowTextW(l.hwnd, wide(&title(mode)).as_ptr()) };
        }
        l.mode = mode;
        l.seg_x.to(mode as f32);
        // The list as it shows now cross-fades into the new one, which settles into place.
        let lay = layout(l);
        l.old_list = l.canvas.as_mut().map(|c| {
            let r = rect(0, lay.list_top as i32, (lay.list_w as i32).min(c.w), c.h - lay.list_top as i32);
            (r, c.copy_out(r))
        });
        l.list_a.snap(0.0);
        l.list_a.launch(1.0);
        l.scroll.snap(0.0);
        l.sel.clear();
        rebuild(l);
        anim::wake(Client::List);
    }
}

pub(super) fn rebuild(l: &mut Lib) {
    let q = crate::common::search::Query::new(&l.query);
    let searching = !l.query.trim().is_empty();
    let mode = l.mode;
    // When the selected note goes (archived, deleted), select its neighbour, not the top.
    let was = primary_index(l);
    let was_id = l.sel.last().copied();
    let before: std::collections::HashMap<u64, usize> = l.rows.iter().enumerate().map(|(i, r)| (r.id, i)).collect();
    let same_list = l.listed == (mode, l.query.clone()) && l.drag.is_none();
    l.listed = (mode, l.query.clone());
    let prev: std::collections::HashMap<u64, usize> = if editor_focused(l) { l.rows.iter().enumerate().map(|(i, r)| (r.id, i)).collect() } else { Default::default() };
    // The last list's titles and previews, taken again for the notes they're still right
    // for: a search keystroke made every row anew (a preview is most of a row's cost).
    let mut last: std::collections::HashMap<u64, Row> = std::mem::take(&mut l.rows).into_iter().map(|r| (r.id, r)).collect();
    let mut rows: Vec<Row> = app::store(|s| {
        s.d.notes
            .iter()
            .filter(|n| match mode {
                1 => n.archived,
                2 => true,
                _ => !n.archived,
            })
            .filter_map(|n| row_again(n, &q, searching, last.remove(&n.id)))
            .collect()
    });
    // What's left of the last list is the rows that have gone from it.
    if searching && !editor_focused(l) {
        // Best matches first; among equals, the most recently edited.
        rows.sort_by(|a, b| b.score.cmp(&a.score).then(b.updated.cmp(&a.updated)));
    } else if mode == 0 {
        // Notes: the deck's own order, which you arrange by dragging rows here.
        rows.sort_by(|a, b| a.order.cmp(&b.order).then(b.created.cmp(&a.created)));
    } else if editor_focused(l) {
        rows.sort_by_key(|r| prev.get(&r.id).copied().unwrap_or(0));
    } else {
        rows.sort_by_key(|r| std::cmp::Reverse(r.updated));
    }
    // Nothing here? Count the matches in the other list, to offer them.
    l.elsewhere = if searching && rows.is_empty() && mode != 2 {
        app::store(|s| s.d.notes.iter().filter(|n| n.archived == (mode == 0) && q.score(&title_of(&n.text), &search_rest(&n.text)).is_some()).count())
    } else {
        0
    };
    l.rows = rows;
    if same_list {
        close_gaps(l, &before, last, was_id);
    } else {
        l.leaving.clear();
        l.arriving.clear();
    }
    l.sel.retain(|id| l.rows.iter().any(|r| r.id == *id));
    if l.sel.is_empty() {
        let near = was.unwrap_or(0).min(l.rows.len().saturating_sub(1));
        if let Some(r) = l.rows.get(near) {
            l.sel.push(r.id);
            l.anchor = Some(r.id);
            // The selection stays where it was (the row below closes up into it) and turns
            // its colour; the card cross-fades.
            swap_detail(l);
        }
    }
    if let Some(i) = primary_index(l) {
        let lay = layout(l);
        l.sel_y.to(lay.list_top + i as f32 * lay.row_h);
    }
    clamp_scroll(l);
    anim::wake(Client::List);
}

/// Rows left the list (`gone`, from what was at `before`) or came into it: those that
/// stayed move from where they were drawn to their new places, closing the gaps or
/// opening them; a row that left fades where it was, and one that came fades in. A
/// reordering (the same rows) moves nothing here: the drag that made it already has.
fn close_gaps(l: &mut Lib, before: &std::collections::HashMap<u64, usize>, gone: std::collections::HashMap<u64, Row>, was: Option<u64>) {
    let now: Vec<u64> = l.rows.iter().map(|r| r.id).collect();
    let (moved, came) = shifts(before, &now);
    if gone.is_empty() && came.is_empty() {
        return;
    }
    let lay = layout(l);
    for (id, rows) in moved {
        let from = l.offs.get(&id).map_or(0.0, |sp| sp.value) + rows as f32 * lay.row_h;
        let mut sp = Spring::px(from, GAP);
        sp.to(0.0);
        l.offs.insert(id, sp);
    }
    for id in came {
        let mut sp = Spring::fade(0.0, (1.0, 0.24));
        sp.launch(1.0);
        l.arriving.insert(id, (ARRIVE_WAIT_S, sp));
    }
    for (id, row) in gone {
        let Some(&at) = before.get(&id) else { continue };
        l.offs.remove(&id);
        l.leaving.push(Leaving { row, at, fade: FadeOut::new(1.0, LEAVE_S), selected: was == Some(id) });
        l.hold = LEAVE_S;
    }
    anim::wake(Client::List);
}

/// For the list `now` (note ids) after the list whose places were `before`: each row that
/// moved, with how many rows below its new place it was (up is negative), and the rows
/// that came.
fn shifts(before: &std::collections::HashMap<u64, usize>, now: &[u64]) -> (Vec<(u64, isize)>, Vec<u64>) {
    let mut moved = Vec::new();
    let mut came = Vec::new();
    for (i, id) in now.iter().enumerate() {
        match before.get(id) {
            Some(&was) if was != i => moved.push((*id, was as isize - i as isize)),
            Some(_) => {}
            None => came.push(*id),
        }
    }
    (moved, came)
}

/// Whether rows are still closing or opening a gap (the inline editor waits for them).
pub(super) fn gaps_moving(l: &Lib) -> bool {
    !l.leaving.is_empty() || !l.arriving.is_empty() || l.offs.values().any(|sp| !sp.settled())
}

/// Note `n`'s row, or None when a search is on and it doesn't match.
pub(super) fn row_of(n: &Note, q: &crate::common::search::Query, searching: bool) -> Option<Row> {
    row_again(n, q, searching, None)
}

/// `row_of`, with the title and preview of `last` (the note's row in the last list) when
/// they're still the note's: made the same way from the same text (as long, at the same
/// last edit). The rest is worked out again: the match, and the time, which moves on.
pub(super) fn row_again(n: &Note, q: &crate::common::search::Query, searching: bool, last: Option<Row>) -> Option<Row> {
    let (title, body) = match last.filter(|r| r.updated == n.updated && r.text_len == n.text.len()) {
        Some(r) => (r.title, Some(r.body)),
        None => (without_objects(title_of(&n.text)), None),
    };
    let m = if searching {
        let rest = search_rest(&n.text);
        let m = q.score(&title, &rest)?;
        let snippet = m.body_at.map(|at| without_objects(crate::common::search::snippet(&rest, at, 80)));
        let marks = snippet.as_deref().map(|s| q.marks(s)).unwrap_or_default();
        (m.score, m.title, snippet, marks)
    } else {
        (0, Vec::new(), None, Vec::new())
    };
    Some(Row {
        id: n.id,
        title,
        off_deck: n.off_deck,
        order: n.order,
        body: body.unwrap_or_else(|| without_objects(body_of(&n.text))),
        text_len: n.text.len(),
        time: relative_time(n.updated),
        created: n.created,
        updated: n.updated,
        color: n.color,
        archived: n.archived,
        score: m.0,
        hits: m.1,
        snippet: m.2,
        snippet_hits: m.3,
    })
}

pub(super) fn primary_index(l: &Lib) -> Option<usize> {
    let id = *l.sel.last()?;
    l.rows.iter().position(|r| r.id == id)
}

pub(super) fn row_y(l: &Lib, lay: &Layout, i: usize) -> f32 {
    let id = l.rows[i].id;
    if let Some(dr) = l.drag.as_ref().filter(|dr| dr.active && dr.id == id) {
        return lay.list_top + dr.y - l.scroll.value;
    }
    let off = l.offs.get(&id).map_or(0.0, |sp| sp.value);
    lay.list_top + i as f32 * lay.row_h - l.scroll.value + off
}

/// Rows can be dragged into a new order in the Notes view (not while searching).
pub(super) fn can_drag(l: &Lib) -> bool {
    l.mode == 0 && l.query.is_empty()
}

/// Follow the pointer with the dragged row; the others spring aside to make room.
pub(super) fn drag_to(l: &mut Lib, y: i32) {
    let lay = layout(l);
    let n = l.rows.len();
    let Some(dr) = l.drag.as_mut() else { return };
    if !dr.active {
        if (y - dr.start_y).abs() <= (5.0 * l.s) as i32 {
            return;
        }
        dr.active = true;
        l.hover_a.to(0.0);
    }
    let max = (n.saturating_sub(1)) as f32 * lay.row_h;
    let raw = y as f32 - lay.list_top + l.scroll.value - dr.grab;
    // A little give past the ends of the list, then resistance.
    let band = |over: f32| over * 0.35;
    dr.y = if raw < 0.0 { band(raw) } else if raw > max { max + band(raw - max) } else { raw };
    dr.to = ((dr.y / lay.row_h).round().max(0.0) as usize).min(n.saturating_sub(1));
    let (from, to, id, top) = (dr.from, dr.to, dr.id, dr.y);
    for (k, r) in l.rows.iter().enumerate() {
        if r.id == id {
            continue;
        }
        let shift = if from < to && k > from && k <= to {
            -lay.row_h
        } else if to < from && k >= to && k < from {
            lay.row_h
        } else {
            0.0
        };
        l.offs.entry(r.id).or_insert_with(|| Spring::px(0.0, (0.9, 0.26))).to(shift);
    }
    l.sel_y.snap(lay.list_top + top);
    anim::wake(Client::List);
}

/// Let go: the new order is saved (the deck follows) and the row settles into its slot.
pub(super) fn drop_drag(l: &mut Lib) -> Action {
    let Some(dr) = l.drag.take() else { return Action::None };
    if !dr.active {
        return Action::None;
    }
    let lay = layout(l);
    let mut ids: Vec<u64> = l.rows.iter().map(|r| r.id).collect();
    let id = ids.remove(dr.from);
    ids.insert(dr.to, id);
    for sp in l.offs.values_mut() {
        sp.snap(0.0);
    }
    let mut settle = Spring::px(dr.y - dr.to as f32 * lay.row_h, (0.86, 0.3));
    settle.to(0.0);
    l.offs.insert(id, settle);
    l.sel_y.snap(lay.list_top + dr.to as f32 * lay.row_h);
    anim::wake(Client::List);
    if dr.to == dr.from { Action::None } else { Action::Reorder(ids) }
}

pub(super) fn cancel_drag(l: &mut Lib) {
    if l.drag.take().is_some_and(|dr| dr.active) {
        for sp in l.offs.values_mut() {
            sp.to(0.0);
        }
        if let Some(i) = primary_index(l) {
            let lay = layout(l);
            l.sel_y.to(lay.list_top + i as f32 * lay.row_h);
        }
        anim::wake(Client::List);
    }
}

pub(super) fn max_scroll(l: &Lib, lay: &Layout) -> f32 {
    (l.rows.len() as f32 * lay.row_h + 16.0 * l.s - (lay.h as f32 - lay.list_top)).max(0.0)
}

pub(super) fn clamp_scroll(l: &mut Lib) {
    let lay = layout(l);
    let max = max_scroll(l, &lay);
    l.scroll.to(l.scroll.target.clamp(0.0, max));
}

pub(super) fn ensure_visible(l: &mut Lib, i: usize) {
    let lay = layout(l);
    let top = i as f32 * lay.row_h;
    let view = lay.h as f32 - lay.list_top - 8.0 * l.s;
    let mut t = l.scroll.target;
    if top < t {
        t = top;
    } else if top + lay.row_h > t + view {
        t = top + lay.row_h - view;
    }
    l.scroll.to(t.clamp(0.0, max_scroll(l, &lay)));
}

pub(super) fn select_row(l: &mut Lib, i: usize, ctrl: bool, shift: bool) {
    let Some(id) = l.rows.get(i).map(|r| r.id) else { return };
    let lay = layout(l);
    let before = l.sel.last().copied();
    if ctrl {
        if let Some(pos) = l.sel.iter().position(|x| *x == id) {
            if l.sel.len() > 1 {
                l.sel.remove(pos);
            }
        } else {
            l.sel.push(id);
        }
        l.anchor = Some(id);
    } else if shift {
        let a = l.anchor.and_then(|a| l.rows.iter().position(|r| r.id == a)).unwrap_or(i);
        let (lo, hi) = (a.min(i), a.max(i));
        l.sel = l.rows[lo..=hi].iter().map(|r| r.id).filter(|x| *x != id).collect();
        l.sel.push(id);
    } else {
        l.sel = vec![id];
        l.anchor = Some(id);
    }
    if let Some(pi) = primary_index(l) {
        let target = lay.list_top + pi as f32 * lay.row_h;
        if before.is_none() {
            l.sel_y.snap(target);
        } else {
            l.sel_y.to(target);
        }
    }
    if before != l.sel.last().copied() || l.sel.len() > 1 {
        swap_detail(l);
    }
    anim::wake(Client::List);
}

pub(super) fn move_selection(l: &mut Lib, delta: i32) {
    if l.rows.is_empty() {
        return;
    }
    let cur = primary_index(l).map(|i| i as i32).unwrap_or(-1);
    let next = (cur + delta).clamp(0, l.rows.len() as i32 - 1) as usize;
    select_row(l, next, false, key_down(VK_SHIFT));
    ensure_visible(l, next);
}

pub(super) fn card_screen_rect(l: &Lib) -> Option<(RECT, f32)> {
    let lay = layout(l);
    if !lay.wide {
        return None;
    }
    let mut p = POINT { x: lay.card.x as i32, y: lay.card.y as i32 };
    unsafe { ClientToScreen(l.hwnd, &mut p) };
    Some((RECT { left: p.x, top: p.y, right: p.x + lay.card.w as i32, bottom: p.y + lay.card.h as i32 }, lay.card.r))
}

pub(super) fn open_selected(l: &Lib) -> Action {
    match l.sel.last() {
        Some(id) if l.sel.len() == 1 => Action::Open(*id, card_screen_rect(l)),
        Some(id) => Action::Open(*id, None),
        None => Action::None,
    }
}

pub(super) fn click(l: &mut Lib, hit: Hit) -> Action {
    let sel = l.sel.clone();
    match hit {
        Hit::New => Action::New,
        Hit::Settings => Action::Settings,
        // The selected notes, in the list's order.
        Hit::Share => Action::Share(l.rows.iter().filter(|r| sel.contains(&r.id)).map(|r| r.id).collect()),
        Hit::Card => open_selected(l),
        Hit::OpenHere => in_window(l).map_or(Action::None, Action::OpenHere),
        Hit::Archive => {
            let all_archived = l.rows.iter().filter(|r| sel.contains(&r.id)).all(|r| r.archived);
            Action::Archive(sel, !all_archived)
        }
        Hit::Delete => Action::Delete(sel),
        Hit::Swatch(k) => match l.sel.last() {
            Some(id) => Action::Color(*id, theme::ORDER[k]),
            None => Action::None,
        },
        Hit::Close => Action::Close,
        Hit::Min => Action::Min,
        Hit::Max => Action::MaxToggle,
        _ => Action::None,
    }
}

#[cfg(test)]
mod tests {
    use super::shifts;

    fn places(ids: &[u64]) -> std::collections::HashMap<u64, usize> {
        ids.iter().enumerate().map(|(i, id)| (*id, i)).collect()
    }

    #[test]
    fn rows_below_one_that_left_close_up_from_where_they_were() {
        // 3 deleted: 4 and 5, a row lower than they are now, slide up a row.
        let (moved, came) = shifts(&places(&[1, 2, 3, 4, 5]), &[1, 2, 4, 5]);
        assert_eq!(moved, vec![(4, 1), (5, 1)]);
        assert!(came.is_empty());
        // Two gone at once, one above the other.
        let (moved, _) = shifts(&places(&[1, 2, 3, 4, 5]), &[2, 4, 5]);
        assert_eq!(moved, vec![(2, 1), (4, 2), (5, 2)]);
    }

    #[test]
    fn a_row_that_came_back_opens_a_gap_for_itself() {
        // 3 restored: it fades in where the rows below it were, and they slide down.
        let (moved, came) = shifts(&places(&[1, 2, 4, 5]), &[1, 2, 3, 4, 5]);
        assert_eq!(moved, vec![(4, -1), (5, -1)]);
        assert_eq!(came, vec![3]);
    }
}
