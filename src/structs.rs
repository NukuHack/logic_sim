use std::ops::{Add, AddAssign, Div, Mul, Neg, Sub};

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq)]
pub struct Vec2 {
	pub x: f32,
	pub y: f32,
}

impl Default for Vec2 {
	fn default() -> Self {
		Self::ZERO
	}
}

impl Vec2 {
	pub const ZERO: Vec2 = Vec2 { x: 0.0, y: 0.0 };

	pub fn new(x: f32, y: f32) -> Self {
		Self { x, y }
	}

	pub fn splat(both: f32) -> Self {
		Self { x: both, y: both }
	}

	pub fn max(&self, other: Self) -> Self {
		Self { x: self.x.max(other.x), y: self.x.max(other.y) }
	}
}

// convert to-from basic types

impl Vec2 {
	pub fn to_tuple(self) -> (f32, f32) {
		(self.x, self.y)
	}

	pub fn to_arr(self) -> [f32; 2] {
		[self.x, self.y]
	}
}

// 2. Add `From`/`Into` conversions for tuples/arrays
impl From<(f32, f32)> for Vec2 {
	fn from((x, y): (f32, f32)) -> Self {
		Self { x, y }
	}
}

impl From<[f32; 2]> for Vec2 {
	fn from([x, y]: [f32; 2]) -> Self {
		Self { x, y }
	}
}

// for basic operations in code like " + "

impl Add for Vec2 {
	type Output = Self;

	fn add(self, other: Self) -> Self {
		Self::new(self.x + other.x, self.y + other.y)
	}
}

impl AddAssign for Vec2 {
	fn add_assign(&mut self, other: Self) {
		*self = Self { x: self.x + other.x, y: self.y + other.y };
	}
}

// (unary minus) - useful for reversing direction
impl Neg for Vec2 {
	type Output = Self;
	fn neg(self) -> Self {
		Self::new(-self.x, -self.y)
	}
}

impl Sub for Vec2 {
	type Output = Self;

	fn sub(self, other: Self) -> Self {
		Self::new(self.x - other.x, self.y - other.y)
	}
}

impl Mul<f32> for Vec2 {
	type Output = Self;

	fn mul(self, scalar: f32) -> Self {
		Self::new(self.x * scalar, self.y * scalar)
	}
}

// Also support scalar multiplication in reverse order (f32 * Vec2)
impl Mul<Vec2> for f32 {
	type Output = Vec2;

	fn mul(self, vec: Vec2) -> Vec2 {
		Vec2::new(vec.x * self, vec.y * self)
	}
}

// Support vector * vector component-wise multiplication
impl Mul for Vec2 {
	type Output = Self;

	fn mul(self, other: Self) -> Self {
		Self::new(self.x * other.x, self.y * other.y)
	}
}

// Support vector / f32
impl Div<f32> for Vec2 {
	type Output = Self;

	fn div(self, scalar: f32) -> Self {
		Self::new(self.x / scalar, self.y / scalar)
	}
}

// Also support the other way f32 / vector
impl Div<Vec2> for f32 {
	type Output = Vec2;

	fn div(self, vec: Vec2) -> Vec2 {
		Vec2::new(vec.x / self, vec.y / self)
	}
}

// Vec2 / Vec2 (component-wise) for completeness
impl Div for Vec2 {
	type Output = Self;
	fn div(self, other: Self) -> Self {
		Self::new(self.x / other.x, self.y / other.y)
	}
}

// complex math

impl Vec2 {
	pub fn magnitude(&self) -> f32 {
		(self.x * self.x + self.y * self.y).sqrt()
	}

	pub fn length(&self) -> f32 {
		self.magnitude()
	}

	pub fn magnitude_sq(&self) -> f32 {
		self.x * self.x + self.y * self.y
	}

	pub fn normalize(&self) -> Self {
		let mag = self.magnitude();
		if mag == 0.0 {
			Self::ZERO
		} else {
			*self / mag
		}
	}

	pub fn dot(&self, other: &Self) -> f32 {
		self.x * other.x + self.y * other.y
	}

	pub fn cross(&self, other: &Self) -> f32 {
		self.x * other.y - self.y * other.x
	}

	pub fn lerp(&self, other: &Self, t: f32) -> Self {
		Self { x: self.x + (other.x - self.x) * t, y: self.y + (other.y - self.y) * t }
	}
}
