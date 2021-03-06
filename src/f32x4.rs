use std::mem;
use cgmath::*;

#[derive(Clone, Copy, Debug)]
#[simd]
pub struct f32x4(pub f32, pub f32, pub f32, pub f32);

impl f32x4 {
    #[inline]
    pub fn broadcast(v: f32) -> f32x4 {
        f32x4(v, v, v, v)
    }

    #[inline]
    pub fn range_x() -> f32x4 {
        f32x4(0., 1., 0., 1.)
    }

    #[inline]
    pub fn range_y() -> f32x4 {
        f32x4(0., 0., 1., 1.)
    }

    /// casts a each f32 to its bit forms as u32
    /// this is numerically useless, but used for bit twiddling
    /// inside of the library
    #[inline]
    pub fn to_bit_u32x4(self) -> u32x4 {
        unsafe { mem::transmute(self) }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct f32x4_vec2(pub [f32x4; 2]);

impl f32x4_vec2 {
    #[inline]
    pub fn broadcast(v: Vector2<f32>) -> f32x4_vec2 {
        f32x4_vec2([f32x4::broadcast(v.x),
                    f32x4::broadcast(v.y)])
    }

    #[inline]
    pub fn range(x: f32, y: f32, xs: f32, ys: f32) -> f32x4_vec2 {
        f32x4_vec2([f32x4::range_x() * f32x4::broadcast(xs) + f32x4::broadcast(x),
                    f32x4::range_y() * f32x4::broadcast(ys) + f32x4::broadcast(y)])
    }

    #[inline]
    pub fn dot(self, rhs: f32x4_vec2) -> f32x4 {
        self.0[0] * rhs.0[0] + self.0[1] * rhs.0[1]
    }
}

#[derive(Clone, Copy, Debug)]
pub struct f32x4_vec3(pub [f32x4; 3]);

impl f32x4_vec3 {
    #[inline]
    pub fn broadcast(v: Vector3<f32>) -> f32x4_vec3 {
        f32x4_vec3([f32x4::broadcast(v.x),
                    f32x4::broadcast(v.y),
                    f32x4::broadcast(v.z)])
    }

    #[inline]
    pub fn range(x: f32, y: f32, xs: f32, ys: f32) -> f32x4_vec3 {
        f32x4_vec3([f32x4::range_x() * f32x4::broadcast(xs) + f32x4::broadcast(x),
                    f32x4::range_y() * f32x4::broadcast(ys) + f32x4::broadcast(y),
                    f32x4::broadcast(0.)])
    }

    #[inline]
    pub fn dot(self, rhs: f32x4_vec3) -> f32x4 {
        self.0[0] * rhs.0[0] + self.0[1] * rhs.0[1] + self.0[2] * rhs.0[2]
    }
}

#[derive(Clone, Copy, Debug)]
#[simd]
pub struct u32x4(pub u32, pub u32, pub u32, pub u32);

impl u32x4 {
    #[inline]
    pub fn and_self(self) -> u32 {
        let (a, b) = self.split();
        (a & b).and_self()
    }

    #[inline]
    pub fn or_self(self) -> u32 {
        let (a, b) = self.split();
        (a | b).or_self()
    }

    #[inline]
    pub fn split(self) -> (u32x2, u32x2) {
        unsafe { mem::transmute(self) }
    }
}


#[derive(Clone, Copy, Debug)]
#[simd]
pub struct u32x2(pub u32, pub u32);

impl u32x2 {
    #[inline]
    pub fn and_self(self) -> u32 {
        let (a, b) = self.split();
        a & b
    }

    #[inline]
    pub fn or_self(self) -> u32 {
        let (a, b) = self.split();
        a | b
    }

    #[inline]
    pub fn split(self) -> (u32, u32) {
        unsafe { mem::transmute(self) }
    }
}