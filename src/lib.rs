#![feature(simd, unboxed_closures, core, slice_patterns, std_misc)]
#![allow(non_camel_case_types)]

extern crate image;
extern crate genmesh;
extern crate cgmath;
extern crate threadpool;
extern crate num_cpus;

use std::num::Float;
use std::sync::{Arc, Future};
use std::sync::mpsc::channel;
use std::iter::range_step_inclusive;
use std::fmt::Debug;

use threadpool::ThreadPool;
use image::{GenericImage, ImageBuffer, Rgba};
use cgmath::*;
use genmesh::{Triangle, MapVertex};

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
    pool: ThreadPool
}

impl<P: Copy+Send+'static> Frame<P> {
    pub fn new(width: u32, height: u32, p: P) -> Frame<P> {
        Frame {
            width: width,
            height: height,
            tile: (0..(height / 32_)).map(
                |_| (0..(width / 32_)).map(
                    |_| Future::from_value(Box::new(TileGroup::new(p)))
                ).collect()
            ).collect(),
            pool: ThreadPool::new(num_cpus::get())
        }
    }

    pub fn clear(&mut self, p: P) {
        use std::mem;
        for row in self.tile.iter_mut() {
            for tile in row.iter_mut() {
                let (tx, rx) = channel();
                let mut new = Future::from_receiver(rx);
                mem::swap(tile, &mut new);
                self.pool.execute(move || {
                    let mut t = new.get();
                    t.clear(p);
                    tx.send(t).unwrap();
                });
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

        let mut commands: Vec<Vec<Vec<(Triangle<Vector3<f32>>, Triangle<T>)>>> =
            (0..(h / 32_)).map( 
                |_| (0..(w / 32_)).map(
                    |_| Vec::with_capacity(256)
                ).collect()
            ).collect();

        let fragment = Arc::new(fragment);

        for or in poly {
            let t = or.clone().map_vertex(|v| {
                let v = v.position();
                Vector4::new(v[0], v[1], v[2], v[3])
            });

            let clip = t.map_vertex(|v| v.truncate().div_s(v.w) );

            /*if !is_backface(clip) {
                continue;
            }*/

            let clip2 = clip.map_vertex(|v| Vector2::new(v.x * wh + wh, v.y * hh + hh));
            let max_x = clip2.x.x.ceil().partial_max(clip2.y.x.ceil().partial_max(clip2.z.x.ceil()));
            let min_x = clip2.x.x.floor().partial_min(clip2.y.x.floor().partial_min(clip2.z.x.floor()));
            let max_y = clip2.x.y.ceil().partial_max(clip2.y.y.ceil().partial_max(clip2.z.y.ceil()));
            let min_y = clip2.x.y.floor().partial_min(clip2.y.y.floor().partial_min(clip2.z.y.floor()));

            let min_x = (max(min_x as i32, 0) as u32) & (0xFFFFFFFF & !0x1F_);
            let min_y = (max(min_y as i32, 0) as u32) & (0xFFFFFFFF & !0x1F_);
            let max_x = min(max_x as u32, w-0x1F_);
            let max_y = min(max_y as u32, h-0x1F_);

            for y in range_step_inclusive(min_y, max_y, 32_) {
                for x in range_step_inclusive(min_x, max_x, 32_) {
                    let ix = (x / 32_) as usize;
                    let iy = (y / 32_) as usize;
                    commands[ix][iy].push((clip.clone(), or.clone()));

                    if commands[ix][iy].len() == commands[ix][iy].capacity() {
                        let tile = &mut self.tile[ix][iy];
                        let fragment = fragment.clone();
                        let (tx, rx) = channel();
                        let mut new = Future::from_receiver(rx);
                        std::mem::swap(&mut new, tile);

                        let mut tile_poly = Vec::with_capacity(256);
                        std::mem::swap(&mut tile_poly, &mut commands[ix][iy]);
                        self.pool.execute(move || {
                            let mut t = new.get();
                            let pos = Vector2::new((x as f32 - wh) * scale.x, (y as f32 - hh) * scale.y);
                            for (clip, ref or) in tile_poly.into_iter() {
                                let clip3 = Vector3::new(clip.x.z, clip.y.z, clip.z.z);
                                let bary = Barycentric::new(clip.map_vertex(|v| v.truncate()));
                                t.raster(pos, scale, &clip3, &bary, or, &*fragment);
                            }
                            tx.send(t).unwrap();
                        });
                    }
                }
            }
        }

        for (x, (row, row_poly)) in self.tile.iter_mut().zip(commands.into_iter()).enumerate() {
            for (y, (tile, tile_poly)) in row.iter_mut().zip(row_poly.into_iter()).enumerate() {
                if tile_poly.len() == 0 {
                    continue;
                }

                let x = x as u32;
                let y = y as u32;

                let fragment = fragment.clone();
                let (tx, rx) = channel();
                let mut new = Future::from_receiver(rx);
                std::mem::swap(&mut new, tile);

                self.pool.execute(move || {
                    let mut t = new.get();
                    let pos = Vector2::new(((x*32) as f32 - wh) * scale.x, ((y*32) as f32 - hh) * scale.y);
                    for (clip, ref or) in tile_poly.into_iter() {
                        let clip3 = Vector3::new(clip.x.z, clip.y.z, clip.z.z);
                        let bary = Barycentric::new(clip.map_vertex(|v| v.truncate()));
                        t.raster(pos, scale, &clip3, &bary, or, &*fragment);
                    }
                    tx.send(t).unwrap();
                });
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
                let (tx_self, rx) = channel();
                let mut new = Future::from_receiver(rx);
                mem::swap(tile, &mut new);
                let (tx_src, rx) = channel();
                let mut src = Future::from_receiver(rx);
                mem::swap(src_tile, &mut src);
                let pixel = pixel.clone();
                self.pool.execute(move || {
                    let mut dst = new.get();
                    let src = src.get();
                    dst.map(&src, &*pixel);
                    tx_self.send(dst).unwrap();
                    tx_src.send(src).unwrap();
                });
            }
        }
    }

    pub fn flush(&mut self) {
        for row in self.tile.iter_mut() {
            for tile in row.iter_mut() {
                tile.get();
            }
        }
    }
}

impl Frame<Rgba<u8>> {
    pub fn to_image(&mut self) -> ImageBuffer<Rgba<u8>, Vec<u8>> {
        let mut buffer = ImageBuffer::new(self.width, self.height);

        for (x, row) in self.tile.iter_mut().enumerate() {
            for (y, tile) in row.iter_mut().enumerate() {
                let t = tile.get();
                t.write((x*32_) as u32, (y*32_) as u32, &mut buffer);
            }
        }

        buffer
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
