//! Walking the element tree and drawing each shape.

use super::*;

pub(super) fn walk(s: &str, mut pos: usize, cv: &mut Canvas, base: Mat, g: &Gradients) {
    struct Frame {
        m: Mat,
        st: Style,
        /// Inside `defs`, `symbol`, `clipPath`, `mask`, `pattern`, `text`...: not drawn.
        skip: bool,
    }
    let mut stack: Vec<Frame> = alloc::vec![Frame {
        m: base,
        st: Style::root(),
        skip: false
    }];
    let mut budget = MAX_SEGMENTS;
    let mut elements = 0usize;
    while let Some(t) = next_tag(s, &mut pos) {
        elements += 1;
        if elements > MAX_ELEMENTS {
            return;
        }
        if t.close && !t.open {
            if stack.len() > 1 {
                stack.pop();
            }
            continue;
        }
        let Some(top) = stack.last() else { return };
        let mut m = top.m;
        if let Some(tr) = t.get("transform") {
            m = m.mul(parse_transform(tr));
        }
        let st = style_of(&top.st, &t, g);
        let hidden = t.get("display").is_some_and(|d| d.trim() == "none")
            || t.get("visibility").is_some_and(|d| d.trim() == "hidden")
            || t.get("style").is_some_and(|c| c.contains("display:none"));
        let container_skip = matches!(
            t.name,
            "defs"
                | "symbol"
                | "clipPath"
                | "mask"
                | "pattern"
                | "marker"
                | "text"
                | "title"
                | "desc"
                | "style"
                | "metadata"
                | "linearGradient"
                | "radialGradient"
                | "filter"
                | "script"
                | "foreignObject"
        );
        let skip = top.skip || hidden || container_skip || t.get("clip-path").is_some() && false;
        let shape = if skip {
            None
        } else {
            shape_of(&t, &m, &mut budget)
        };
        if let Some(polys) = shape {
            draw_shape(cv, &polys, &t, &m, &st);
        }
        if t.open && !t.close {
            if stack.len() >= MAX_DEPTH {
                return;
            }
            stack.push(Frame { m, st, skip });
        }
    }
}

/// The outline of a shape element in device space, as `(polygon, closed)` pieces.
pub(super) fn shape_of(t: &Tag, m: &Mat, budget: &mut usize) -> Option<Vec<(Vec<Pt>, bool)>> {
    let f = |k: &str| t.get(k).and_then(|v| length(v, 100 << 16)).unwrap_or(0);
    match t.name {
        "path" => {
            let mut out = Vec::new();
            flatten_path(t.get("d")?, m, &mut out, budget);
            Some(out)
        }
        "rect" => {
            let (w, h) = (f("width"), f("height"));
            if w <= 0 || h <= 0 {
                return None;
            }
            let (mut rx, mut ry) = (f("rx"), f("ry"));
            if rx == 0 {
                rx = ry;
            }
            if ry == 0 {
                ry = rx;
            }
            Some(alloc::vec![(
                rect_poly(m, f("x"), f("y"), w, h, rx, ry),
                true
            )])
        }
        "circle" => {
            let r = f("r");
            (r > 0).then(|| alloc::vec![(ellipse_poly(m, f("cx"), f("cy"), r, r), true)])
        }
        "ellipse" => {
            let (rx, ry) = (f("rx"), f("ry"));
            (rx > 0 && ry > 0)
                .then(|| alloc::vec![(ellipse_poly(m, f("cx"), f("cy"), rx, ry), true)])
        }
        "line" => Some(alloc::vec![(
            alloc::vec![m.apply(f("x1"), f("y1")), m.apply(f("x2"), f("y2"))],
            false
        )]),
        "polyline" | "polygon" => {
            let mut pts = Vec::new();
            let mut s = t.get("points")?;
            let mut nums = Vec::new();
            while let Some((v, r)) = num(skip_sep(s)) {
                nums.push(v);
                s = r;
                if nums.len() > 20_000 {
                    break;
                }
            }
            for c in nums.as_chunks::<2>().0 {
                pts.push(m.apply(c[0], c[1]));
            }
            Some(alloc::vec![(pts, t.name == "polygon")])
        }
        _ => None,
    }
}

pub(super) fn draw_shape(cv: &mut Canvas, shape: &[(Vec<Pt>, bool)], t: &Tag, m: &Mat, st: &Style) {
    let _ = t;
    let op = st.opacity;
    if let Paint::Color(c) = st.fill {
        // A line has no area to fill.
        let polys: Vec<Vec<Pt>> = shape
            .iter()
            .filter(|(p, _)| p.len() > 2)
            .map(|(p, _)| p.clone())
            .collect();
        fill(cv, &polys, st.even_odd, c, st.fill_op * op / 255);
    }
    if let Paint::Color(c) = st.stroke {
        let half = (st.stroke_w * m.scale() >> 16) >> 9; // half the width, 24.8 device units
        let mut polys = Vec::new();
        for (p, closed) in shape {
            stroke_polys(p, *closed, half, &mut polys);
        }
        fill(cv, &polys, false, c, st.stroke_op * op / 255);
    }
}
