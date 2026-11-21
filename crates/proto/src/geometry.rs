use crate::model::{Anchor, Overlay};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Size {
    pub w: u32,
    pub h: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

/// Swaps the displayed dimensions for quarter-turn rotations.
pub fn rotated_size(size: Size, rotation: u16) -> Size {
    if matches!(rotation, 90 | 270) {
        Size { w: size.h, h: size.w }
    } else {
        size
    }
}

/// Position of the anchor inside a box of `size`, relative to its top-left corner.
pub fn anchor_point(size: Size, anchor: Anchor) -> Point {
    use Anchor::*;
    let (fx, fy) = match anchor {
        TopLeft => (0, 0),
        Top => (1, 0),
        TopRight => (2, 0),
        Left => (0, 1),
        Center => (1, 1),
        Right => (2, 1),
        BottomLeft => (0, 2),
        Bottom => (1, 2),
        BottomRight => (2, 2),
    };
    Point { x: (size.w as i64 * fx / 2) as i32, y: (size.h as i64 * fy / 2) as i32 }
}

fn scaled(media: Size, factor: f64) -> (f64, f64) {
    (media.w as f64 * factor, media.h as f64 * factor)
}

/// Overlay size in logical pixels: explicit width/height (aspect preserved, fit inside when both),
/// else `media * scale`; then shrunk uniformly to fit the output.
pub fn overlay_size(output: Size, media: Size, ov: &Overlay) -> Size {
    let media = rotated_size(Size { w: media.w.max(1), h: media.h.max(1) }, ov.rotation);
    let factor = match (ov.width, ov.height) {
        (Some(w), Some(h)) => (w as f64 / media.w as f64).min(h as f64 / media.h as f64),
        (Some(w), None) => w as f64 / media.w as f64,
        (None, Some(h)) => h as f64 / media.h as f64,
        (None, None) => ov.scale,
    };
    let (w, h) = scaled(media, factor);
    let fit = (output.w as f64 / w).min(output.h as f64 / h).min(1.0);
    Size { w: ((w * fit).round() as u32).max(1), h: ((h * fit).round() as u32).max(1) }
}

/// How far past each monitor edge an overlay with `overflow` may go: half its size, so at least half stays visible.
pub fn overflow_slack(size: Size) -> (i32, i32) {
    ((size.w / 2) as i32, (size.h / 2) as i32)
}

/// Final rectangle of an overlay on its output, clamped to stay fully inside, or, with `overflow`, to keep at least
/// half of it inside.
pub fn place(output: Size, media: Size, ov: &Overlay) -> Rect {
    let size = overlay_size(output, media, ov);
    let (out_a, own_a) = (anchor_point(output, ov.anchor), anchor_point(size, ov.anchor));
    let (sx, sy) = if ov.overflow { overflow_slack(size) } else { (0, 0) };
    let max_x = output.w.saturating_sub(size.w) as i32 + sx;
    let max_y = output.h.saturating_sub(size.h) as i32 + sy;
    Rect {
        x: (out_a.x - own_a.x + ov.x).clamp(-sx, max_x),
        y: (out_a.y - own_a.y + ov.y).clamp(-sy, max_y),
        w: size.w,
        h: size.h,
    }
}

/// The part of `rect` inside an output of `size` (what is drawn of an overflowing overlay).
pub fn visible(output: Size, rect: Rect) -> Rect {
    let (x0, y0) = (rect.x.max(0), rect.y.max(0));
    let x1 = (rect.x + rect.w as i32).min(output.w as i32);
    let y1 = (rect.y + rect.h as i32).min(output.h as i32);
    Rect { x: x0, y: y0, w: (x1 - x0).max(1) as u32, h: (y1 - y0).max(1) as u32 }
}

/// Inverse of the position step of `place`: the `(x, y)` offset that puts a box of `rect_size`
/// with its top-left corner at `top_left` when using `anchor`.
pub fn offset_for(output: Size, rect_size: Size, top_left: Point, anchor: Anchor) -> (i32, i32) {
    let (out_a, own_a) = (anchor_point(output, anchor), anchor_point(rect_size, anchor));
    (top_left.x - (out_a.x - own_a.x), top_left.y - (out_a.y - own_a.y))
}

#[cfg(test)]
mod tests {
    use super::*;

    const OUT: Size = Size { w: 1000, h: 600 };
    const MEDIA: Size = Size { w: 200, h: 100 };

    const ALL: [Anchor; 9] = {
        use Anchor::*;
        [TopLeft, Top, TopRight, Left, Center, Right, BottomLeft, Bottom, BottomRight]
    };

    #[test]
    fn rotated_size_swaps_quarter_turns() {
        assert_eq!(rotated_size(MEDIA, 0), MEDIA);
        assert_eq!(rotated_size(MEDIA, 90), Size { w: 100, h: 200 });
        assert_eq!(rotated_size(MEDIA, 270), Size { w: 100, h: 200 });
    }

    fn ov(anchor: Anchor, x: i32, y: i32) -> Overlay {
        Overlay { anchor, x, y, ..Overlay::default() }
    }

    #[test]
    fn nine_anchors() {
        let expected = [(0, 0), (400, 0), (800, 0), (0, 250), (400, 250), (800, 250), (0, 500), (400, 500), (800, 500)];
        for (a, (x, y)) in ALL.into_iter().zip(expected) {
            assert_eq!(place(OUT, MEDIA, &ov(a, 0, 0)), Rect { x, y, w: 200, h: 100 }, "{a:?}");
        }
    }

    #[test]
    fn offset_follows_screen_direction() {
        let r = place(OUT, MEDIA, &ov(Anchor::BottomRight, -20, -30));
        assert_eq!((r.x, r.y), (780, 470));
    }

    #[test]
    fn overflow_keeps_half_inside() {
        let o = Overlay { overflow: true, ..ov(Anchor::BottomLeft, -500, 500) };
        let r = place(OUT, MEDIA, &o);
        assert_eq!((r.x, r.y), (-100, 550));
        assert_eq!(visible(OUT, r), Rect { x: 0, y: 550, w: 100, h: 50 });
        let inside = place(OUT, MEDIA, &Overlay { overflow: false, ..o });
        assert_eq!((inside.x, inside.y), (0, 500));
        assert_eq!(visible(OUT, inside), inside);
    }

    #[test]
    fn position_is_clamped() {
        let r = place(OUT, MEDIA, &ov(Anchor::TopLeft, -50, 9999));
        assert_eq!((r.x, r.y), (0, 500));
        let r = place(OUT, MEDIA, &ov(Anchor::BottomRight, 50, 50));
        assert_eq!((r.x, r.y), (800, 500));
    }

    #[test]
    fn size_from_scale_width_height_and_box() {
        let size = |o: Overlay| overlay_size(OUT, MEDIA, &o);
        assert_eq!(size(Overlay { scale: 2.0, ..Default::default() }), Size { w: 400, h: 200 });
        assert_eq!(size(Overlay { width: Some(100), scale: 5.0, ..Default::default() }), Size { w: 100, h: 50 });
        assert_eq!(size(Overlay { height: Some(300), ..Default::default() }), Size { w: 600, h: 300 });
        let boxed = Overlay { width: Some(100), height: Some(100), ..Default::default() };
        assert_eq!(size(boxed), Size { w: 100, h: 50 });
    }

    #[test]
    fn oversized_overlay_shrinks_to_output() {
        let s = overlay_size(OUT, Size { w: 4000, h: 1200 }, &Overlay::default());
        assert_eq!(s, Size { w: 1000, h: 300 });
        assert_eq!(overlay_size(OUT, Size { w: 0, h: 0 }, &Overlay::default()), Size { w: 1, h: 1 });
    }

    #[test]
    fn offset_for_inverts_place() {
        for a in ALL {
            let r = place(OUT, MEDIA, &ov(a, 17, -9));
            let (x, y) = offset_for(OUT, Size { w: r.w, h: r.h }, Point { x: r.x, y: r.y }, a);
            assert_eq!(place(OUT, MEDIA, &ov(a, x, y)), r, "{a:?}");
        }
    }
}
