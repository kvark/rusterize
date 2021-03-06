#![feature(simd, unboxed_closures, core, slice_patterns, step_by)]
#![allow(non_camel_case_types)]

extern crate image;
extern crate genmesh;
extern crate cgmath;
extern crate fibe;
extern crate snowstorm;
extern crate future_pulse;
extern crate pulse;
extern crate vec_map;

use std::sync::Arc;
use std::fmt::Debug;
use std::cell::UnsafeCell;

use fibe::{Frontend, task, ResumableTask, WaitState, Schedule, IntoTask};
use image::{GenericImage, ImageBuffer, Rgba};
use cgmath::*;
use genmesh::{Triangle, MapVertex};
use future_pulse::*;
use pulse::*;
use snowstorm::channel::*;
use vec_map::*;

pub use tile::{TileGroup, Tile, Raster};
use vmath::Dot;
use f32x8::f32x8x8;
pub use pipeline::{Fragment, Vertex, Mapping};
pub use interpolate::{Flat, Interpolate};

mod interpolate;
mod pipeline;
mod f32x4;
pub mod f32x8;
mod vmath;
pub mod tile;


#[cfg(dump)]
fn dump(idx: usize, frame: &Frame) {
    use std::old_io::File;
    // Save the image output just incase the test fails
    let mut fout = File::create(&Path::new("dump").join(format!("{:05}.png", idx))).unwrap();
    let _= image::ImageRgba8(frame.frame.clone()).save(&mut fout, image::PNG);
}

#[inline]
pub fn is_backface(v: Triangle<Vector3<f32>>)-> bool {
    let e0 = v.z - v.x;
    let e1 = v.z - v.y;
    let normal = e1.cross(&e0);
    Vector3::new(0., 0., 1.).dot(normal) >= 0.
}

#[derive(Clone, Copy, Debug)]
pub struct Barycentric {
    pub v0: Vector2<f32>,
    pub v1: Vector2<f32>,
    pub base: Vector2<f32>,
    inv_denom: f32
}

#[derive(Debug)]
pub struct BarycentricCoordinate {
    pub u: f32,
    pub v: f32,
}

impl BarycentricCoordinate {
    /// check if the point is inside the triangle
    #[inline]
    pub fn inside(&self) -> bool {
        (self.u >= 0.) && (self.v >= 0.) && ((self.u + self.v) <= 1.)
    }

    #[inline]
    pub fn weights(&self) -> [f32; 3] {
        [1. - self.u - self.v, self.u, self.v]
    }
}

impl Barycentric {
    pub fn new(t: Triangle<Vector2<f32>>) -> Barycentric {
        let v0 = t.y - t.x;
        let v1 = t.z - t.x;

        let d00 = v0.dot(v0);
        let d01 = v0.dot(v1);
        let d11 = v1.dot(v1);

        let inv_denom = 1. / (d00 * d11 - d01 * d01);

        Barycentric {
            v0: v0,
            v1: v1,
            base: t.x,
            inv_denom: inv_denom
        }
    }

    #[inline]
    pub fn coordinate(&self, p: Vector2<f32>) -> BarycentricCoordinate {
        let p = Vector2::new(p.x, p.y);
        let v2 = p - self.base;

        let d00 = self.v0.dot(self.v0);
        let d01 = self.v0.dot(self.v1);
        let d02 = self.v0.dot(v2);
        let d11 = self.v1.dot(self.v1);
        let d12 = self.v1.dot(v2);

        let u = (d11 * d02 - d01 * d12) * self.inv_denom;
        let v = (d00 * d12 - d01 * d02) * self.inv_denom;

        BarycentricCoordinate {
            u: u,
            v: v
        }
    }

    #[inline]
    pub fn coordinate_f32x4(&self, p: Vector2<f32>, s: Vector2<f32>) -> [f32x4::f32x4; 2] {
        use f32x4::{f32x4, f32x4_vec2};
        let p = Vector2::new(p.x, p.y);
        let v2 = p - self.base;

        let v0 = f32x4_vec2::broadcast(self.v0);
        let v1 = f32x4_vec2::broadcast(self.v1);
        let v2 = f32x4_vec2::range(v2.x, v2.y, s.x, s.y);

        let d00 = v0.dot(v0);
        let d01 = v0.dot(v1);
        let d02 = v0.dot(v2);
        let d11 = v1.dot(v1);
        let d12 = v1.dot(v2);

        let inv_denom = f32x4::broadcast(self.inv_denom);

        [(d11 * d02 - d01 * d12) * inv_denom,
         (d00 * d12 - d01 * d02) * inv_denom]
    }

    #[inline]
    pub fn coordinate_f32x8x8(&self, p: Vector2<f32>, s: Vector2<f32>) -> [f32x8::f32x8x8; 2] {
        use f32x8::{f32x8x8, f32x8x8_vec2};

        let v2 = f32x8x8_vec2::range(p, s) - f32x8x8_vec2::broadcast(self.base);

        let d00 = self.v0.dot(self.v0);
        let d01 = self.v0.dot(self.v1);
        let d02 = self.v0.dot(v2);
        let d11 = self.v1.dot(self.v1);
        let d12 = self.v1.dot(v2);

        let inv_denom = f32x8x8::broadcast(self.inv_denom);

        [(d02 * d11 - d12 * d01) * inv_denom,
         (d12 * d00 - d02 * d01) * inv_denom]
    }

    /// a fast to check to tell if a tile is inside of the triangle or not
    #[inline]
    pub fn tile_fast_check(&self, p: Vector2<f32>, s: Vector2<f32>) -> bool {
        use f32x4::{f32x4};
        let [u, v] = self.coordinate_f32x4(p, s);
        let uv = f32x4::broadcast(1.) - (u + v);
        let mask = u.to_bit_u32x4().and_self() |
                   v.to_bit_u32x4().and_self() |
                   uv.to_bit_u32x4().and_self();

        mask & 0x8000_0000 != 0
    }

    #[inline]
    pub fn tile_covered(&self, p: Vector2<f32>, s: Vector2<f32>) -> bool {
        use f32x4::{f32x4};
        let [u, v] = self.coordinate_f32x4(p, s);
        let uv = f32x4::broadcast(1.) - (u + v);
        let mask = u.to_bit_u32x4().or_self() |
                   v.to_bit_u32x4().or_self() |
                   uv.to_bit_u32x4().or_self();

        mask & 0x8000_0000 != 0
    }
}

pub struct Frame<P> {
    pub width: u32,
    pub height: u32,
    pub tile: Vec<Vec<Future<Box<TileGroup<P>>>>>,
    pool: Frontend
}

struct RasterWorker<P: Send, T: Send+Sync, F> {
    tile: Option<Box<TileGroup<P>>>,
    polygons: Receiver<(Triangle<Vector3<f32>>, Triangle<T>)>,
    pos: Vector2<f32>,
    scale: Vector2<f32>,
    fragment: Arc<F>,
    result: Option<future_pulse::Set<Box<TileGroup<P>>>>
}

impl<T: Send+Sync, P: Send+Copy, F, O> ResumableTask for RasterWorker<P, T, F>
    where F: Fragment<O, Color=P>+Send+Sync,
          T: Interpolate<Out=O>+Send+Sync+Debug

{
    fn resume(&mut self, _: &mut Schedule) -> WaitState {
        let mut tile = self.tile.take().unwrap();

        while let Some(&(ref clip, ref or)) = self.polygons.try_recv() {
            let z = Vector3::new(clip.x.z, clip.y.z, clip.z.z);
            let bary = Barycentric::new(clip.map_vertex(|v| v.truncate()));
            tile.raster(self.pos, self.scale, &z, &bary, or, &*self.fragment);
        }

        if self.polygons.closed() {
            self.result.take().unwrap().set(tile);
            WaitState::Completed
        } else {
            self.tile = Some(tile);
            WaitState::Pending(self.polygons.signal())
        }
    }
}

impl<P: Copy+Sync+Send+'static> Frame<P> {
    pub fn new(width: u32, height: u32, p: P) -> Frame<P> {
        Frame {
            width: width,
            height: height,
            tile: (0..(height / 32_)).map(
                |_| (0..(width / 32_)).map(
                    |_| Future::from_value(Box::new(TileGroup::new(p)))
                ).collect()
            ).collect(),
            pool: Frontend::new()
        }
    }

    pub fn clear(&mut self, p: P) {
        use std::mem;
        for row in self.tile.iter_mut() {
            for tile in row.iter_mut() {
                let (mut new, set) = Future::new();
                mem::swap(tile, &mut new);
                let signal = new.signal();
                task(move |_| {
                    let mut t = new.get();
                    t.clear(p);
                    set.set(t);
                }).after(signal).start(&mut self.pool);
            }
        }
    }

    pub fn raster<S, F, T, O>(&mut self, poly: S, fragment: F)
        where S: Iterator<Item=Triangle<T>>,
              T: Clone + Interpolate<Out=O> + FetchPosition + Send + Sync + 'static + Debug,
              F: Fragment<O, Color=P> + Send + Sync + 'static {

        use std::cmp::{min, max};
        let h = self.height;
        let w = self.width;
        let (hf, wf) = (h as f32, w as f32);
        let (hh, wh) = (hf/2., wf/2.);
        let scale = Vector2::new(hh.recip(), wh.recip());

        let fragment = Arc::new(fragment);

        let mut queue = VecMap::new();
        let width = self.width as usize;
        let index = |x, y| {width * y + x};

        let mut command = |x, y, t| {
            let i = index(x, y);
            if queue.get(&i).is_none() {
                use std::mem;
                let (tx, rx) = channel();
                let (mut future, set) = Future::new();
                let fragment = fragment.clone();
                mem::swap(&mut self.tile[x as usize][y as usize], &mut future);
                let signal = future.signal();

                task(move |sched| {
                    let wh = wh;
                    let hh = hh;
                    let scale = scale;
                    let signal = rx.signal();
                    RasterWorker {
                        tile: Some(future.get()),
                        polygons: rx,
                        scale: scale,
                        pos: Vector2::new(((x*32) as f32 - wh) * scale.x,
                                          ((y*32) as f32 - hh) * scale.y),
                        fragment: fragment,
                        result: Some(set)
                    }.after(signal).start(sched);
                }).after(signal).start(&mut self.pool);
                queue.insert(i, tx);
            }

            queue.get_mut(&i).unwrap().send(t);
        };

        for or in poly {
            let t = or.clone().map_vertex(|v| {
                let v = v.position();
                Vector4::new(v[0], v[1], v[2], v[3])
            });

            let clip = t.map_vertex(|v| v.truncate().div_s(v.w) );

            if is_backface(clip) {
                continue;
            }

            let clip2 = clip.map_vertex(|v| Vector2::new(v.x * wh + wh, v.y * hh + hh));
            let max_x = clip2.x.x.ceil().partial_max(clip2.y.x.ceil().partial_max(clip2.z.x.ceil()));
            let min_x = clip2.x.x.floor().partial_min(clip2.y.x.floor().partial_min(clip2.z.x.floor()));
            let max_y = clip2.x.y.ceil().partial_max(clip2.y.y.ceil().partial_max(clip2.z.y.ceil()));
            let min_y = clip2.x.y.floor().partial_min(clip2.y.y.floor().partial_min(clip2.z.y.floor()));

            let min_x = (max(min_x as i32, 0) as u32) & (0xFFFFFFFF & !0x1F_);
            let min_y = (max(min_y as i32, 0) as u32) & (0xFFFFFFFF & !0x1F_);
            let max_x = min(max_x as u32, w-0x1F_);
            let max_y = min(max_y as u32, h-0x1F_);

            for y in (min_y..max_y+1).step_by(32) {
                for x in (min_x..max_x+1).step_by(32) {
                    let ix = (x / 32_) as usize;
                    let iy = (y / 32_) as usize;
                    command(ix, iy, (clip.clone(), or.clone()));
                }
            }
        }
    }

    pub fn map<S, F>(&mut self, src: &mut Frame<S>, pixel: F)
        where F: Mapping<S, Out=P> + Sized + Send + Sync + 'static,
              S: Send + Sync + 'static + Copy {
        use std::mem;

        assert!(src.width == self.width);
        assert!(src.height == self.height);

        let pixel = Arc::new(pixel);

        for (row, src_row) in self.tile.iter_mut().zip(src.tile.iter_mut()) {
            for (tile, src_tile) in row.iter_mut().zip(src_row.iter_mut()) {
                let (mut new, tx_self) = Future::new();
                mem::swap(tile, &mut new);
                let (mut src, tx_src) = Future::new();
                mem::swap(src_tile, &mut src);
                let pixel = pixel.clone();
                let (s0, s1) = (new.signal(), src.signal());
                task(move |_| {
                    let mut dst = new.get();
                    let src = src.get();
                    dst.map(&src, &*pixel);
                    tx_self.set(dst);
                    tx_src.set(src);
                }).after(s0).after(s1).start(&mut self.pool);
            }
        }
    }

    pub fn flush(&mut self) {
        for row in self.tile.iter_mut() {
            for tile in row.iter_mut() {
                tile.signal().wait().unwrap();
            }
        }
    }
}

impl Frame<Rgba<u8>> {
    pub fn into_image(&mut self, img: ImageBuffer<Rgba<u8>, Vec<u8>>) -> ImageBuffer<Rgba<u8>, Vec<u8>> {
        use std::mem;
        let buffer = UnsafeCell::new(img);
        let mut signals = Vec::new();

        for (x, row) in self.tile.iter_mut().enumerate() {
            for (y, tile) in row.iter_mut().enumerate() {
                let (mut new, tx_self) = Future::new();
                mem::swap(tile, &mut new);
                let buff: &mut ImageBuffer<_, Vec<_>> = unsafe { mem::transmute(buffer.get()) };
                let signal = new.signal();
                signals.push(task(move |_| {
                    let t = new.get();
                    t.write((x*32_) as u32, (y*32_) as u32, buff);
                    tx_self.set(t);
                }).after(signal).start(&mut self.pool));
            }
        }

        Barrier::new(&signals).wait().unwrap();
        unsafe { buffer.into_inner() }
    }

    pub fn to_image(&mut self) -> ImageBuffer<Rgba<u8>, Vec<u8>> {
        let img = ImageBuffer::new(self.width, self.height);
        self.into_image(img)
    }
}


pub trait FetchPosition {
    fn position(&self) -> [f32; 4];
}

impl FetchPosition for [f32; 4] {
    fn position(&self) -> [f32; 4] { *self }
}

impl<A> FetchPosition for ([f32; 4], A) {
    fn position(&self) -> [f32; 4] { self.0 }
}

impl<A, B> FetchPosition for ([f32; 4], A, B) {
    fn position(&self) -> [f32; 4] { self.0 }
}

impl<A, B, C> FetchPosition for ([f32; 4], A, B, C) {
    fn position(&self) -> [f32; 4] { self.0 }
}

impl<A, B, C, D> FetchPosition for ([f32; 4], A, B, C, D) {
    fn position(&self) -> [f32; 4] { self.0 }
}

impl<A, B, C, D, E> FetchPosition for ([f32; 4], A, B, C, D, E) {
    fn position(&self) -> [f32; 4] { self.0 }
}

impl<A, B, C, D, E, F> FetchPosition for ([f32; 4], A, B, C, D, E, F) {
    fn position(&self) -> [f32; 4] { self.0 }
}

impl<A, B, C, D, E, F, G> FetchPosition for ([f32; 4], A, B, C, D, E, F, G) {
    fn position(&self) -> [f32; 4] { self.0 }
}
