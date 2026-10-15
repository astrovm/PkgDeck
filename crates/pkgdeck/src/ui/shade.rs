//! Shaded fills: gradients through many stops and lit, glassy shapes,
//! with edges as smooth as egui's own shapes. A mesh egui didn't build has
//! hard, stair-stepped edges, so every fill here ends in a one-pixel band
//! fading to clear, as egui's shapes do.

use eframe::egui::{
    epaint::{tessellator::path, Mesh, RoundedRect},
    vec2, Color32, CornerRadius, Painter, Pos2, Rect, Vec2,
};

/// Which way a gradient runs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Axis {
    /// Top to bottom.
    Down,
    /// Left to right.
    Across,
}

impl Axis {
    fn of(self, point: Pos2) -> f32 {
        match self {
            Self::Down => point.y,
            Self::Across => point.x,
        }
    }
    /// `point` moved to `value` along this axis.
    fn with(self, point: Pos2, value: f32) -> Pos2 {
        match self {
            Self::Down => Pos2::new(point.x, value),
            Self::Across => Pos2::new(value, point.y),
        }
    }
    fn span(self, rect: Rect) -> (f32, f32) {
        match self {
            Self::Down => (rect.top(), rect.height()),
            Self::Across => (rect.left(), rect.width()),
        }
    }
}

/// `a` blended toward `b`, alpha included.
fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let channel = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
    Color32::from_rgba_premultiplied(
        channel(a.r(), b.r()),
        channel(a.g(), b.g()),
        channel(a.b(), b.b()),
        channel(a.a(), b.a()),
    )
}

/// The colour at `t` of a gradient through `stops`, given as places from 0
/// to 1 in order.
pub fn color_at(stops: &[(f32, Color32)], t: f32) -> Color32 {
    let Some(&(first_at, first)) = stops.first() else {
        return Color32::TRANSPARENT;
    };
    if t <= first_at {
        return first;
    }
    for pair in stops.windows(2) {
        let ((from, a), (to, b)) = (pair[0], pair[1]);
        if t <= to {
            return mix(a, b, (t - from) / (to - from).max(1e-6));
        }
    }
    stops[stops.len() - 1].1
}

/// The outline of `rect` rounded by `radius`, without repeated points.
pub fn outline(rect: Rect, radius: CornerRadius) -> Vec<Pos2> {
    let mut points = Vec::new();
    path::rounded_rectangle(&mut points, RoundedRect::new(rect, radius));
    tidy(points)
}

/// The outline of the ellipse filling `rect`.
pub fn ellipse(rect: Rect) -> Vec<Pos2> {
    let radius = rect.size() / 2.0;
    // Enough points that the curve never shows a corner.
    let count = ((radius.max_elem() * 1.5) as usize).clamp(24, 96);
    (0..count)
        .map(|i| {
            let angle = i as f32 / count as f32 * std::f32::consts::TAU;
            rect.center() + vec2(angle.cos() * radius.x, angle.sin() * radius.y)
        })
        .collect()
}

/// `points` without points that sit on the one before.
fn tidy(mut points: Vec<Pos2>) -> Vec<Pos2> {
    points.dedup_by(|a, b| a.distance_sq(*b) < 1e-6);
    while points.len() > 1 && points[0].distance_sq(points[points.len() - 1]) < 1e-6 {
        points.pop();
    }
    points
}

fn centroid(points: &[Pos2]) -> Pos2 {
    let sum = points.iter().fold(Vec2::ZERO, |sum, p| sum + p.to_vec2());
    (sum / points.len().max(1) as f32).to_pos2()
}

/// The way out of the convex `points` at each point, as long as it takes
/// to step one unit away from both edges that meet there.
fn normals(points: &[Pos2]) -> Vec<Vec2> {
    let center = centroid(points);
    let count = points.len();
    let edge = |i: usize| {
        let (a, b) = (points[i], points[(i + 1) % count]);
        let along = (b - a).normalized();
        let normal = vec2(along.y, -along.x);
        if normal.dot(a.lerp(b, 0.5) - center) < 0.0 {
            -normal
        } else {
            normal
        }
    };
    (0..count)
        .map(|i| {
            let (before, after) = (edge((i + count - 1) % count), edge(i));
            let sum = before + after;
            let cos = 1.0 + before.dot(after);
            if cos < 0.1 {
                after
            } else {
                sum / cos
            }
        })
        .collect()
}

/// One screen pixel, in points: how wide egui fades its edges.
fn pixel(painter: &Painter) -> f32 {
    1.0 / painter.ctx().pixels_per_point().max(0.1)
}

/// Fills the convex `points` with `color` at each place, and fades its
/// edge over `feather` points. The inside is cut along `cuts`, lines across
/// `axis` where the colour bends, so it follows `color` exactly between
/// them.
fn fill(
    painter: &Painter,
    points: &[Pos2],
    feather: f32,
    axis: Axis,
    cuts: &[f32],
    color: &dyn Fn(Pos2) -> Color32,
) {
    if points.len() < 3 {
        return;
    }
    // Points where the edge crosses a cut, so it bends with the inside.
    let points = &split(points, axis, cuts);
    let normals = normals(points);
    let half = feather / 2.0;
    let inner: Vec<Pos2> = points
        .iter()
        .zip(&normals)
        .map(|(p, n)| *p - *n * half)
        .collect();
    let mut mesh = Mesh::default();
    // The inside, slab by slab between the cuts.
    let (low, high) = inner
        .iter()
        .map(|p| axis.of(*p))
        .fold((f32::MAX, f32::MIN), |(lo, hi), v| (lo.min(v), hi.max(v)));
    let mut edges: Vec<f32> = cuts
        .iter()
        .copied()
        .filter(|c| *c > low && *c < high)
        .collect();
    edges.extend([low, high]);
    edges.sort_by(f32::total_cmp);
    edges.dedup_by(|a, b| (*a - *b).abs() < 1e-3);
    for pair in edges.windows(2) {
        let (from, to) = (pair[0], pair[1]);
        let slab = clip(&inner, axis, from, to);
        if slab.len() >= 3 {
            // Colours from just inside the slab, so a hard edge at a cut
            // stays hard on both sides.
            let nudge = ((to - from) * 0.25).min(1e-3);
            let inside = |p: Pos2| color(axis.with(p, axis.of(p).clamp(from + nudge, to - nudge)));
            fan(&mut mesh, &slab, &inside);
        }
    }
    // The edge, fading to clear outward.
    if feather > 0.0 {
        let base = mesh.vertices.len() as u32;
        for ((p, n), inside) in points.iter().zip(&normals).zip(&inner) {
            mesh.colored_vertex(*inside, color(*inside));
            mesh.colored_vertex(*p + *n * half, Color32::TRANSPARENT);
        }
        let count = points.len() as u32;
        for i in 0..count {
            let j = (i + 1) % count;
            let (a, b, c, d) = (
                base + 2 * i,
                base + 2 * i + 1,
                base + 2 * j,
                base + 2 * j + 1,
            );
            mesh.add_triangle(a, b, c);
            mesh.add_triangle(b, d, c);
        }
    }
    painter.add(mesh);
}

/// `points` with a point added wherever an edge crosses one of `cuts`.
fn split(points: &[Pos2], axis: Axis, cuts: &[f32]) -> Vec<Pos2> {
    let mut out = Vec::with_capacity(points.len() + cuts.len() * 2);
    for (i, &a) in points.iter().enumerate() {
        let b = points[(i + 1) % points.len()];
        out.push(a);
        let (va, vb) = (axis.of(a), axis.of(b));
        let mut crossings: Vec<f32> = cuts
            .iter()
            .filter(|c| (**c > va.min(vb) + 1e-3) && (**c < va.max(vb) - 1e-3))
            .map(|c| (c - va) / (vb - va))
            .collect();
        crossings.sort_by(f32::total_cmp);
        out.extend(crossings.into_iter().map(|t| a.lerp(b, t)));
    }
    out
}

/// The convex `points` cut down to between `from` and `to` along `axis`.
fn clip(points: &[Pos2], axis: Axis, from: f32, to: f32) -> Vec<Pos2> {
    let mut kept = points.to_vec();
    for (edge, keep_above) in [(from, true), (to, false)] {
        let inside = |p: Pos2| {
            let v = axis.of(p);
            if keep_above {
                v >= edge
            } else {
                v <= edge
            }
        };
        let mut next = Vec::with_capacity(kept.len() + 2);
        for (i, &a) in kept.iter().enumerate() {
            let b = kept[(i + 1) % kept.len()];
            if inside(a) {
                next.push(a);
            }
            if inside(a) != inside(b) {
                let t = (edge - axis.of(a)) / (axis.of(b) - axis.of(a));
                next.push(a.lerp(b, t));
            }
        }
        kept = next;
        if kept.is_empty() {
            break;
        }
    }
    kept
}

fn fan(mesh: &mut Mesh, points: &[Pos2], color: &dyn Fn(Pos2) -> Color32) {
    let base = mesh.vertices.len() as u32;
    for &p in points {
        mesh.colored_vertex(p, color(p));
    }
    for i in 2..points.len() as u32 {
        mesh.add_triangle(base, base + i - 1, base + i);
    }
}

/// A gradient through `stops` along `axis` over `rect`, rounded by
/// `radius`. Square fills keep their hard edges, as they sit on whole
/// pixels; rounded ones fade theirs.
pub fn gradient(
    painter: &Painter,
    rect: Rect,
    radius: CornerRadius,
    axis: Axis,
    stops: &[(f32, Color32)],
) {
    if rect.width() <= 0.0 || rect.height() <= 0.0 || stops.is_empty() {
        return;
    }
    let (start, length) = axis.span(rect);
    let cuts: Vec<f32> = stops
        .iter()
        .map(|(at, _)| start + at.clamp(0.0, 1.0) * length)
        .collect();
    let feather = if radius == CornerRadius::ZERO {
        0.0
    } else {
        pixel(painter)
    };
    let color = |p: Pos2| color_at(stops, (axis.of(p) - start) / length);
    fill(
        painter,
        &outline(rect, radius),
        feather,
        axis,
        &cuts,
        &color,
    );
}

/// The convex `points` lit by `color` at each place: rings from the edge
/// in to `center`, so light can pool and fall off any way. For round
/// glassy things: Aqua's lights and gel.
pub fn lit(
    painter: &Painter,
    points: &[Pos2],
    center: Pos2,
    rings: usize,
    color: &dyn Fn(Pos2) -> Color32,
) {
    if points.len() < 3 {
        return;
    }
    let half = pixel(painter) / 2.0;
    let normals = normals(points);
    let edge: Vec<Pos2> = points
        .iter()
        .zip(&normals)
        .map(|(p, n)| *p - *n * half)
        .collect();
    let rings = rings.max(1);
    let count = edge.len() as u32;
    let mut mesh = Mesh::default();
    // Ring 0 is the edge; the last ring is the centre itself.
    for ring in 0..=rings {
        let t = ring as f32 / rings as f32;
        for p in &edge {
            let at = p.lerp(center, t);
            mesh.colored_vertex(at, color(at));
        }
    }
    for ring in 0..rings as u32 {
        for i in 0..count {
            let j = (i + 1) % count;
            let (a, b) = (ring * count + i, ring * count + j);
            let (c, d) = ((ring + 1) * count + i, (ring + 1) * count + j);
            mesh.add_triangle(a, b, c);
            mesh.add_triangle(b, d, c);
        }
    }
    let base = mesh.vertices.len() as u32;
    for ((p, n), inside) in points.iter().zip(&normals).zip(&edge) {
        mesh.colored_vertex(*inside, color(*inside));
        mesh.colored_vertex(*p + *n * half, Color32::TRANSPARENT);
    }
    for i in 0..count {
        let j = (i + 1) % count;
        let (a, b, c, d) = (
            base + 2 * i,
            base + 2 * i + 1,
            base + 2 * j,
            base + 2 * j + 1,
        );
        mesh.add_triangle(a, b, c);
        mesh.add_triangle(b, d, c);
    }
    painter.add(mesh);
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::{pos2, Shape};

    fn rect() -> Rect {
        Rect::from_min_size(pos2(10.0, 20.0), vec2(80.0, 24.0))
    }

    fn mesh_of(paint: impl FnOnce(&Painter)) -> Mesh {
        let shapes = crate::ui::theme::painted(|ui| paint(ui.painter()));
        let mut all = Mesh::default();
        for shape in shapes {
            if let Shape::Mesh(mesh) = shape {
                all.append(std::sync::Arc::unwrap_or_clone(mesh));
            }
        }
        all
    }

    #[test]
    fn colours_between_stops_blend_and_hold_past_the_ends() {
        let stops = [(0.2, Color32::BLACK), (0.6, Color32::WHITE)];
        assert_eq!(color_at(&stops, 0.0), Color32::BLACK);
        assert_eq!(color_at(&stops, 0.2), Color32::BLACK);
        let middle = color_at(&stops, 0.4);
        assert!((127..=128).contains(&middle.r()), "{middle:?}");
        assert_eq!(color_at(&stops, 0.6), Color32::WHITE);
        assert_eq!(color_at(&stops, 1.0), Color32::WHITE);
        assert_eq!(color_at(&[], 0.5), Color32::TRANSPARENT);
        // Two stops at one place make a hard edge.
        let hard = [(0.5, Color32::RED), (0.5, Color32::BLUE)];
        assert_eq!(color_at(&hard, 0.49), Color32::RED);
        assert_eq!(color_at(&hard, 0.51), Color32::BLUE);
    }

    #[test]
    fn outlines_have_no_repeated_points_and_ellipses_stay_inside() {
        for radius in [0, 4, 12, 255] {
            let points = outline(rect(), CornerRadius::same(radius));
            assert!(points.len() >= 4, "{radius}");
            for pair in points.windows(2) {
                assert!(pair[0].distance(pair[1]) > 1e-3, "{radius}");
            }
            assert!(points[0].distance(points[points.len() - 1]) > 1e-3);
            assert!(points.iter().all(|p| rect().expand(0.01).contains(*p)));
        }
        let oval = ellipse(rect());
        assert!(oval.len() >= 24);
        assert!(oval.iter().all(|p| rect().expand(0.01).contains(*p)));
        // Both ends of each axis are reached.
        let xs = oval.iter().map(|p| p.x);
        assert!((xs.clone().fold(f32::MAX, f32::min) - rect().left()).abs() < 0.01);
        assert!((xs.fold(f32::MIN, f32::max) - rect().right()).abs() < 0.1);
    }

    #[test]
    fn normals_point_out_both_ways_round() {
        let square = [
            pos2(0.0, 0.0),
            pos2(10.0, 0.0),
            pos2(10.0, 10.0),
            pos2(0.0, 10.0),
        ];
        let mut backwards = square;
        backwards.reverse();
        for points in [square, backwards] {
            for (p, n) in points.iter().zip(normals(&points)) {
                // A corner steps out diagonally, one unit from each side.
                assert!((n.length() - 2f32.sqrt()).abs() < 1e-4, "{p:?} {n:?}");
                assert!(n.dot(*p - pos2(5.0, 5.0)) > 0.0, "{p:?} {n:?}");
            }
        }
    }

    #[test]
    fn rounded_fills_fade_their_edge_and_square_ones_keep_it() {
        let stops = [
            (0.0, Color32::RED),
            (0.5, Color32::GREEN),
            (1.0, Color32::BLUE),
        ];
        let round = mesh_of(|p| gradient(p, rect(), CornerRadius::same(8), Axis::Down, &stops));
        let clear = |mesh: &Mesh| {
            mesh.vertices
                .iter()
                .filter(|v| v.color == Color32::TRANSPARENT)
                .count()
        };
        assert!(clear(&round) > 8);
        // The fade reaches half a pixel past the shape and no further.
        let bounds = round.calc_bounds();
        assert!(rect().expand(0.51).contains_rect(bounds), "{bounds:?}");
        assert!(bounds.width() > rect().width());
        let square = mesh_of(|p| gradient(p, rect(), CornerRadius::ZERO, Axis::Down, &stops));
        assert_eq!(clear(&square), 0);
        assert_eq!(square.calc_bounds(), rect());
        // A hard edge stays hard: no slab blends across it.
        let hard = [
            (0.0, Color32::RED),
            (0.5, Color32::RED),
            (0.5, Color32::BLUE),
            (1.0, Color32::BLUE),
        ];
        let split = mesh_of(|p| gradient(p, rect(), CornerRadius::ZERO, Axis::Down, &hard));
        for triangle in split.indices.chunks(3) {
            let colours: Vec<Color32> = triangle
                .iter()
                .map(|i| split.vertices[*i as usize].color)
                .collect();
            assert!(colours.iter().all(|c| *c == colours[0]), "{colours:?}");
        }
        // The middle stop lands exactly on its line.
        let middle = rect().center().y;
        assert!(square
            .vertices
            .iter()
            .any(|v| (v.pos.y - middle).abs() < 1e-3 && v.color == Color32::GREEN));
        for v in &square.vertices {
            let t = (v.pos.y - rect().top()) / rect().height();
            let near = |a: Color32, b: Color32| {
                a.to_array()
                    .iter()
                    .zip(b.to_array())
                    .all(|(x, y)| x.abs_diff(y) <= 1)
            };
            assert!(near(v.color, color_at(&stops, t)), "{v:?}");
        }
    }

    #[test]
    fn gradients_across_follow_x() {
        let stops = [(0.0, Color32::BLACK), (1.0, Color32::WHITE)];
        let mesh = mesh_of(|p| gradient(p, rect(), CornerRadius::ZERO, Axis::Across, &stops));
        for v in &mesh.vertices {
            let t = (v.pos.x - rect().left()) / rect().width();
            let want = color_at(&stops, t);
            assert!(v.color.r().abs_diff(want.r()) <= 1, "{v:?} {want:?}");
        }
        // Nothing is drawn for an empty rect or no stops.
        let empty = Rect::from_min_size(pos2(0.0, 0.0), vec2(0.0, 10.0));
        assert!(mesh_of(|p| gradient(p, empty, CornerRadius::ZERO, Axis::Down, &stops)).is_empty());
        assert!(mesh_of(|p| gradient(p, rect(), CornerRadius::ZERO, Axis::Down, &[])).is_empty());
    }

    #[test]
    fn lit_shapes_take_their_colour_where_it_falls_and_fade_their_edge() {
        let ball = Rect::from_center_size(pos2(50.0, 50.0), vec2(14.0, 14.0));
        let top_dark = |p: Pos2| {
            if p.y < 50.0 {
                Color32::BLACK
            } else {
                Color32::WHITE
            }
        };
        let mesh = mesh_of(|p| lit(p, &ellipse(ball), ball.center(), 4, &top_dark));
        assert!(ball.expand(0.51).contains_rect(mesh.calc_bounds()));
        for v in &mesh.vertices {
            if v.color != Color32::TRANSPARENT {
                assert_eq!(v.color, top_dark(v.pos), "{v:?}");
            }
        }
        assert!(mesh.vertices.iter().any(|v| v.pos == ball.center()));
        assert!(mesh_of(|p| lit(p, &[pos2(0.0, 0.0)], pos2(0.0, 0.0), 3, &top_dark)).is_empty());
    }

    #[test]
    fn clipping_keeps_only_what_lies_between() {
        let square = [
            pos2(0.0, 0.0),
            pos2(10.0, 0.0),
            pos2(10.0, 10.0),
            pos2(0.0, 10.0),
        ];
        let slab = clip(&square, Axis::Down, 2.0, 5.0);
        assert!(slab.iter().all(|p| (1.999..=5.001).contains(&p.y)));
        assert_eq!(slab.len(), 4);
        assert!(clip(&square, Axis::Across, 20.0, 30.0).is_empty());
        // Edges crossing a cut gain a point on it, both ways round.
        let split = split(&square, Axis::Down, &[0.0, 4.0, 10.0]);
        assert_eq!(split.len(), 6);
        assert_eq!(split.iter().filter(|p| (p.y - 4.0).abs() < 1e-4).count(), 2);
    }
}
