/*!
A very fast 2D [Delaunay Triangulation](https://en.wikipedia.org/wiki/Delaunay_triangulation) library for Rust.
A port of [Delaunator](https://github.com/mapbox/delaunator).

# Example

```rust
use delaunator::{Point, triangulate};

let points = vec![
    Point { x: 0., y: 0. },
    Point { x: 1., y: 0. },
    Point { x: 1., y: 1. },
    Point { x: 0., y: 1. },
];

let result = triangulate(&points).expect("No triangulation exists.");
println!("{:?}", result.triangles); // [0, 2, 1, 0, 3, 2]
```
*/
extern crate asprim;
extern crate num_traits;

use std::{f64, fmt};
use asprim::AsPrim;
use num_traits::int::PrimInt;

/// Near-duplicate points (where both `x` and `y` only differ within this value)
/// will not be included in the triangulation for robustness.
pub const EPSILON: f64 = f64::EPSILON * 2.0;

/// Represents a 2D point in the input vector.
#[derive(Clone, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl fmt::Debug for Point {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "[{}, {}]", self.x, self.y)
    }
}

impl Point {
    fn dist2(&self, p: &Self) -> f64 {
        let dx = self.x - p.x;
        let dy = self.y - p.y;
        dx * dx + dy * dy
    }

    fn orient(&self, q: &Self, r: &Self) -> bool {
        (q.y - self.y) * (r.x - q.x) - (q.x - self.x) * (r.y - q.y) < 0.0
    }

    fn circumdelta(&self, b: &Self, c: &Self) -> (f64, f64) {
        let dx = b.x - self.x;
        let dy = b.y - self.y;
        let ex = c.x - self.x;
        let ey = c.y - self.y;

        let bl = dx * dx + dy * dy;
        let cl = ex * ex + ey * ey;
        let d = 0.5 / (dx * ey - dy * ex);

        let x = (ey * bl - dy * cl) * d;
        let y = (dx * cl - ex * bl) * d;
        (x, y)
    }

    fn circumradius2(&self, b: &Self, c: &Self) -> f64 {
        let (x, y) = self.circumdelta(b, c);
        x * x + y * y
    }

    fn circumcenter(&self, b: &Self, c: &Self) -> Self {
        let (x, y) = self.circumdelta(b, c);
        Self {
            x: self.x + x,
            y: self.y + y,
        }
    }

    fn in_circle(&self, b: &Self, c: &Self, p: &Self) -> bool {
        let dx = self.x - p.x;
        let dy = self.y - p.y;
        let ex = b.x - p.x;
        let ey = b.y - p.y;
        let fx = c.x - p.x;
        let fy = c.y - p.y;

        let ap = dx * dx + dy * dy;
        let bp = ex * ex + ey * ey;
        let cp = fx * fx + fy * fy;

        dx * (ey * cp - bp * fy) - dy * (ex * cp - bp * fx) + ap * (ex * fy - ey * fx) < 0.0
    }

    fn nearly_equals(&self, p: &Self) -> bool {
        (self.x - p.x).abs() <= EPSILON && (self.y - p.y).abs() <= EPSILON
    }
}

/// Result of the Delaunay triangulation.
pub struct Triangulation<T: AsPrim + PrimInt> {
    /// A vector of point indices where each triple represents a Delaunay triangle.
    /// All triangles are directed counter-clockwise.
    pub triangles: Vec<T>,

    /// A vector of adjacent halfedge indices that allows traversing the triangulation graph.
    ///
    /// `i`-th half-edge in the array corresponds to vertex `triangles[i]`
    /// the half-edge is coming from. `halfedges[i]` is the index of a twin half-edge
    /// in an adjacent triangle (or `EMPTY` for outer half-edges on the convex hull).
    pub halfedges: Vec<T>,

    /// A vector of indices that reference points on the convex hull of the triangulation,
    /// counter-clockwise.
    pub hull: Vec<T>,

    /// Represents the area outside of the triangulation.
    /// Halfedges on the convex hull (which don't have an adjacent halfedge)
    /// will have this value.
    pub empty: T
}

impl<T: AsPrim + PrimInt> Triangulation<T> {
    fn new(points: &[Point]) -> Option<Self> {
        let n = points.len();

        let (i0, i1, i2): (T, T, T) = find_seed_triangle(points)?;
        let center: Point = (&points[i0.as_usize()]).circumcenter(&points[i1.as_usize()], &points[i2.as_usize()]);
        let max_triangles = 2 * n - 5;

        let mut triangulation = Self {
            triangles: Vec::with_capacity(max_triangles * 3),
            halfedges: Vec::with_capacity(max_triangles * 3),
            hull: Vec::new(),
            empty: T::max_value()
        };

        let empty = triangulation.empty;

        triangulation.add_triangle(i0, i1, i2, empty, empty, empty);

        // sort the points by distance from the seed triangle circumcenter
        let mut dists: Vec<_> = points
            .iter()
            .enumerate()
            .map(|(i, point)| (i, center.dist2(point)))
            .collect();

        dists.sort_unstable_by(|&(_, da), &(_, db)| da.partial_cmp(&db).unwrap());

        let mut hull = Hull::new(n, center, i0, i1, i2, points);

        for (k, &(iu, _)) in dists.iter().enumerate() {
            let p = &points[iu];

            // skip near-duplicates
            if k > 0 && p.nearly_equals(&points[dists[k - 1].0]) {
                continue;
            }
            let i: T = iu.as_();
            // skip seed triangle points
            if i == i0 || i == i1 || i == i2 {
                continue;
            }

            // find a visible edge on the convex hull using edge hash
            let (mut e, walk_back) = hull.find_visible_edge(p, points);
            if e == empty {
                continue; // likely a near-duplicate point; skip it
            }

            // add the first triangle from the point
            let t = triangulation.add_triangle(e, i, hull.next(e), empty, empty, hull.out(e));

            // recursively flip triangles from the point until they satisfy the Delaunay condition
            let out = triangulation.legalize(t + 2.as_(), points, &mut hull);
            hull.set_out(i, out);
            hull.set_out(e, t); // keep track of boundary triangles on the hull

            // walk forward through the hull, adding more triangles and flipping recursively
            let mut n = hull.next(e);
            loop {
                let q = hull.next(n);
                if !p.orient(&points[n.as_usize()], &points[q.as_usize()]) {
                    break;
                }
                let t = triangulation.add_triangle(n, i, q, hull.out(i), empty, hull.out(n));
                let out = triangulation.legalize(t + 2.as_(), points, &mut hull);;
                hull.set_out(i, out);
                hull.remove(n);
                n = q;
            }

            // walk backward from the other side, adding more triangles and flipping
            if walk_back {
                loop {
                    let q = hull.prev(e);
                    if !p.orient(&points[q.as_usize()], &points[e.as_usize()]) {
                        break;
                    }
                    let t = triangulation.add_triangle(q, i, e, empty, hull.out(e), hull.out(q));
                    triangulation.legalize(t + 2.as_(), points, &mut hull);
                    hull.set_out(q, t);
                    hull.remove(e);
                    e = q;
                }
            }

            // update the hull indices
            hull.set_prev(i, e);
            hull.set_next(i, n);
            hull.set_prev(n, i);
            hull.set_next(e, i);
            hull.start = e;

            // save the two new edges in the hash table
            hull.hash_edge(p, i);
            hull.hash_edge(&points[e.as_usize()], e);
        }

        // expose hull as a vector of point indices
        let mut e = hull.start;
        loop {
            triangulation.hull.push(e);
            e = hull.next(e);
            if e == hull.start {
                break;
            }
        }

        triangulation.triangles.shrink_to_fit();
        triangulation.halfedges.shrink_to_fit();

        Some(triangulation)
    }

    /// The number of triangles in the triangulation.
    pub fn len(&self) -> usize {
        (self.triangles.len() / 3)
    }

    /// Next halfedge in a triangle.
    pub fn next_halfedge(i: T) -> T {
        if i % 3.as_() == 2.as_() {
            i - 2.as_()
        } else {
            i + 1.as_()
        }
    }

    /// Previous halfedge in a triangle.
    pub fn prev_halfedge(i: T) -> T {
        if i % 3.as_() == 0.as_() {
            i + 2.as_()
        } else {
            i - 1.as_()
        }
    }

    fn twin(&self, halfedge_id: T) -> T {
        self.halfedges[halfedge_id.as_usize()]
    }
    fn set_twin(&mut self, halfedge_id: T, twin_id: T) {
        if halfedge_id != self.empty {
            self.halfedges[halfedge_id.as_usize()] = twin_id
        }
    }

    fn origin(&self, halfedge_id: T) -> T {
        self.triangles[halfedge_id.as_usize()]
    }
    fn set_origin(&mut self, halfedge_id: T, point_id: T) {
        self.triangles[halfedge_id.as_usize()] = point_id;
    }

    fn add_triangle(
        &mut self,
        i0: T,
        i1: T,
        i2: T,
        a: T,
        b: T,
        c: T,
    ) -> T {
        let t: T = self.triangles.len().as_();

        self.triangles.push(i0);
        self.triangles.push(i1);
        self.triangles.push(i2);

        self.halfedges.push(a);
        self.halfedges.push(b);
        self.halfedges.push(c);

        self.set_twin(a, t);
        self.set_twin(b, t + (1).as_());
        self.set_twin(c, t + (2).as_());
        t
    }

    fn legalize(&mut self, a: T, points: &[Point], hull: &mut Hull<T>) -> T {
        let b = self.twin(a);

        // if the pair of triangles doesn't satisfy the Delaunay condition
        // (p1 is inside the circumcircle of [p0, pl, pr]), flip them,
        // then do the same check/flip recursively for the new pair of triangles
        //
        //           pl                    pl
        //          /||\                  /  \
        //       al/ || \bl            al/    \a
        //        /  ||  \              /      \
        //       /  a||b  \    flip    /___ar___\
        //     p0\   ||   /p1   =>   p0\---bl---/p1
        //        \  ||  /              \      /
        //       ar\ || /br             b\    /br
        //          \||/                  \  /
        //           pr                    pr
        //
        let ar = Self::prev_halfedge(a);

        if b == self.empty {
            return ar;
        }

        let al = Self::next_halfedge(a);
        let bl = Self::prev_halfedge(b);

        let p0 = self.origin(ar);
        let pr = self.origin(a);
        let pl = self.origin(al);
        let p1 = self.origin(bl);

        let illegal = (&points[p0.as_usize()]).in_circle(&points[pr.as_usize()], &points[pl.as_usize()], &points[p1.as_usize()]);
        if illegal {
            self.set_origin(a, p1);
            self.set_origin(b, p0);

            let hbl = self.twin(bl);
            let har = self.twin(ar);

            // edge swapped on the other side of the hull (rare); fix the halfedge reference
            if hbl == self.empty {
                hull.fix_halfedge(bl, a);
            }

            self.set_twin(a, hbl);
            self.set_twin(b, har);
            self.set_twin(ar, bl);

            self.set_twin(hbl, a);
            self.set_twin(har, b);
            self.set_twin(bl, ar);

            let br = Self::next_halfedge(b);

            self.legalize(a, points, hull);
            return self.legalize(br, points, hull);
        }
        ar
    }
}

/// data structure for tracking the edges of the advancing convex hull
struct Hull<T: AsPrim + PrimInt> {
    /// maps edge id to prev edge id
    prev: Vec<T>,

    /// maps edge id to next edge id
    next: Vec<T>,

    /// maps point id to outgoing halfedge id
    out: Vec<T>,

    /// angular hull edge hash
    hash: Vec<T>,

    /// starting point of the hull
    start: T,

    /// center of the angular hash
    center: Point,

    empty: T
}

impl<T: PrimInt + AsPrim> Hull<T> {
    fn new(n: usize, center: Point, i0: T, i1: T, i2: T, points: &[Point]) -> Self {
        let hash_len = (n as f64).sqrt() as usize;

        let empty = T::max_value();

        let mut hull = Self {
            prev: vec![T::zero(); n],
            next: vec![T::zero(); n],
            out: vec![T::zero(); n],
            hash: vec![empty; hash_len],
            start: i0,
            center,
            empty
        };

        hull.set_next(i0, i1);
        hull.set_prev(i2, i1);
        hull.set_next(i1, i2);
        hull.set_prev(i0, i2);
        hull.set_next(i2, i0);
        hull.set_prev(i1, i0);

        hull.set_out(i0, 0.as_());
        hull.set_out(i1, 1.as_());
        hull.set_out(i2, 2.as_());

        hull.hash_edge(&points[i0.as_usize()], i0);
        hull.hash_edge(&points[i1.as_usize()], i1);
        hull.hash_edge(&points[i2.as_usize()], i2);

        hull
    }

    fn out(&self, point_id: T) -> T {
        self.out[point_id.as_usize()]
    }
    fn set_out(&mut self, point_id: T, halfedge_id: T) {
        self.out[point_id.as_usize()] = halfedge_id;
    }

    fn prev(&self, point_id: T) -> T {
        self.prev[point_id.as_usize()]
    }
    fn set_prev(&mut self, point_id: T, prev_point_id: T) {
        self.prev[point_id.as_usize()] = prev_point_id;
    }

    fn next(&self, point_id: T) -> T {
        self.next[point_id.as_usize()]
    }
    fn set_next(&mut self, point_id: T, next_point_id: T) {
        self.next[point_id.as_usize()] = next_point_id;
    }

    fn remove(&mut self, point_id: T) {
        let empty = self.empty;
        self.set_next(point_id, empty); // mark as removed
    }

    fn hash_key(&self, p: &Point) -> usize {
        let dx = p.x - self.center.x;
        let dy = p.y - self.center.y;

        let p = dx / (dx.abs() + dy.abs());
        let a = (if dy > 0.0 { 3.0 - p } else { 1.0 + p }) / 4.0; // [0..1]

        let len = self.hash.len();
        (((len as f64) * a).floor() as usize) % len
    }

    fn hash_edge(&mut self, p: &Point, i: T) {
        let key = self.hash_key(p);
        self.hash[key] = i;
    }

    fn find_visible_edge(&self, p: &Point, points: &[Point]) -> (T, bool) {
        let mut start: T = 0.as_();
        let key = self.hash_key(p);
        let len = self.hash.len();
        for j in 0..len {
            start = self.hash[(key + j) % len];
            if start != self.empty && self.next(start) != self.empty {
                break;
            }
        }
        start = self.prev(start);
        let mut e = start;

        while !p.orient(&points[e.as_usize()], &points[self.next(e).as_usize()]) {
            e = self.next(e);
            if e == start {
                return (self.empty, false);
            }
        }
        (e, e == start)
    }

    fn fix_halfedge(&mut self, old_id: T, new_id: T) {
        let mut e = self.start;
        loop {
            if self.out(e) == old_id {
                self.set_out(e, new_id);
                break;
            }
            e = self.next(e);
            if e == self.start {
                break;
            }
        }
    }
}

fn calc_bbox_center(points: &[Point]) -> Point {
    let min_x = points.iter().fold(f64::INFINITY, |acc, p| acc.min(p.x));
    let min_y = points.iter().fold(f64::INFINITY, |acc, p| acc.min(p.y));
    let max_x = points.iter().fold(f64::NEG_INFINITY, |acc, p| acc.max(p.x));
    let max_y = points.iter().fold(f64::NEG_INFINITY, |acc, p| acc.max(p.y));
    Point {
        x: (min_x + max_x) / 2.0,
        y: (min_y + max_y) / 2.0,
    }
}

fn find_closest_point(points: &[Point], p0: &Point) -> Option<usize> {
    let mut min_dist = f64::INFINITY;
    let mut k: usize = 0;
    for (i, p) in points.iter().enumerate() {
        let d = p0.dist2(p);
        if d > 0.0 && d < min_dist {
            k = i;
            min_dist = d;
        }
    }
    if min_dist == f64::INFINITY {
        None
    } else {
        Some(k)
    }
}

fn find_seed_triangle<T: AsPrim + PrimInt>(points: &[Point]) -> Option<(T, T, T)> {
    // pick a seed point close to the center
    let bbox_center = calc_bbox_center(points);
    let i0 = find_closest_point(points, &bbox_center)?;
    let p0 = &points[i0];

    // find the point closest to the seed
    let i1 = find_closest_point(points, p0)?;
    let p1 = &points[i1];

    // find the third point which forms the smallest circumcircle with the first two
    let mut min_radius = f64::INFINITY;
    let mut i2: usize = 0;
    for (i, p) in points.iter().enumerate() {
        if i == i0 || i == i1 {
            continue;
        }
        let r = p0.circumradius2(p1, p);
        if r < min_radius {
            i2 = i;
            min_radius = r;
        }
    }

    if min_radius == f64::INFINITY {
        None
    } else {
        // swap the order of the seed points for counter-clockwise orientation
        Some(if p0.orient(p1, &points[i2]) {
            (i0.as_(), i2.as_(), i1.as_())
        } else {
            (i0.as_(), i1.as_(), i2.as_())
        })
    }
}

/// Triangulate a set of 2D points.
/// Returns `None` if no triangulation exists for the input (e.g. all points are collinear).
pub fn triangulate(points: &[Point]) -> Option<Triangulation<u32>> {
    Triangulation::new(points)
}
