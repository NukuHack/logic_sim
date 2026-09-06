use logic_sim_macros::ConstFromPrimitive;
use std::default::Default;

// Basic test enum
#[derive(ConstFromPrimitive, Default, Debug, PartialEq)]
#[repr(i32)]
enum BasicEnum {
	#[default]
	Unknown = 0,
	First = 1,
	Second = 2,
	Third = 3,
}

#[test]
fn test_basic_enum_all_values() {
	assert_eq!(BasicEnum::from_primitive(0), BasicEnum::Unknown);
	assert_eq!(BasicEnum::from_primitive(1), BasicEnum::First);
	assert_eq!(BasicEnum::from_primitive(2), BasicEnum::Second);
	assert_eq!(BasicEnum::from_primitive(3), BasicEnum::Third);
}

#[test]
fn test_basic_enum_default_for_invalid() {
	assert_eq!(BasicEnum::from_primitive(-1), BasicEnum::Unknown);
	assert_eq!(BasicEnum::from_primitive(4), BasicEnum::Unknown);
	assert_eq!(BasicEnum::from_primitive(i32::MIN), BasicEnum::Unknown);
	assert_eq!(BasicEnum::from_primitive(i32::MAX), BasicEnum::Unknown);
}

// Test with different integer types
#[derive(ConstFromPrimitive, Default, Debug, PartialEq)]
#[repr(u8)]
enum SmallEnum {
	#[default]
	None = 0,
	One = 1,
	Max = 255,
}

#[test]
fn test_u8_enum() {
	assert_eq!(SmallEnum::from_primitive(0), SmallEnum::None);
	assert_eq!(SmallEnum::from_primitive(1), SmallEnum::One);
	assert_eq!(SmallEnum::from_primitive(255), SmallEnum::Max);
	assert_eq!(SmallEnum::from_primitive(2), SmallEnum::None);
	assert_eq!(SmallEnum::from_primitive(254), SmallEnum::None);
}

#[derive(ConstFromPrimitive, Default, Debug, PartialEq)]
#[repr(i64)]
enum LargeEnum {
	#[default]
	Default = -1,
	Negative = -100,
	Positive = 100,
	Max = i64::MAX,
}

#[test]
fn test_i64_enum() {
	assert_eq!(LargeEnum::from_primitive(-1), LargeEnum::Default);
	assert_eq!(LargeEnum::from_primitive(-100), LargeEnum::Negative);
	assert_eq!(LargeEnum::from_primitive(100), LargeEnum::Positive);
	assert_eq!(LargeEnum::from_primitive(i64::MAX), LargeEnum::Max);
	assert_eq!(LargeEnum::from_primitive(0), LargeEnum::Default);
	assert_eq!(LargeEnum::from_primitive(i64::MIN), LargeEnum::Default);
}

// Test with non-sequential values
#[derive(ConstFromPrimitive, Default, Debug, PartialEq)]
#[repr(i32)]
enum SparseEnum {
	#[default]
	Default = 0,
	Ten = 10,
	Hundred = 100,
	Thousand = 1000,
}

#[test]
fn test_sparse_values() {
	assert_eq!(SparseEnum::from_primitive(0), SparseEnum::Default);
	assert_eq!(SparseEnum::from_primitive(10), SparseEnum::Ten);
	assert_eq!(SparseEnum::from_primitive(100), SparseEnum::Hundred);
	assert_eq!(SparseEnum::from_primitive(1000), SparseEnum::Thousand);
	assert_eq!(SparseEnum::from_primitive(50), SparseEnum::Default);
	assert_eq!(SparseEnum::from_primitive(500), SparseEnum::Default);
}

// Test with negative values
#[derive(ConstFromPrimitive, Default, Debug, PartialEq)]
#[repr(i32)]
enum NegativeEnum {
	#[default]
	Default = 0,
	NegOne = -1,
	NegTen = -10,
	NegHundred = -100,
}

#[test]
fn test_negative_values() {
	assert_eq!(NegativeEnum::from_primitive(0), NegativeEnum::Default);
	assert_eq!(NegativeEnum::from_primitive(-1), NegativeEnum::NegOne);
	assert_eq!(NegativeEnum::from_primitive(-10), NegativeEnum::NegTen);
	assert_eq!(NegativeEnum::from_primitive(-100), NegativeEnum::NegHundred);
	assert_eq!(NegativeEnum::from_primitive(-5), NegativeEnum::Default);
	assert_eq!(NegativeEnum::from_primitive(1), NegativeEnum::Default);
}

// Test const context
#[derive(ConstFromPrimitive, Default, Debug, PartialEq)]
#[repr(i32)]
enum ConstEnum {
	#[default]
	Default = 0,
	Value = 42,
}

const CONST_VALUE: ConstEnum = ConstEnum::from_primitive(42);
const CONST_DEFAULT: ConstEnum = ConstEnum::from_primitive(999);

#[test]
fn test_const_context() {
	assert_eq!(CONST_VALUE, ConstEnum::Value);
	assert_eq!(CONST_DEFAULT, ConstEnum::Default);
}

// Test with default not being first variant
#[derive(ConstFromPrimitive, Default, Debug, PartialEq)]
#[repr(i32)]
enum DefaultNotFirst {
	First = 1,
	#[default]
	Default = 0,
	Third = 3,
}

#[test]
fn test_default_not_first() {
	assert_eq!(DefaultNotFirst::from_primitive(1), DefaultNotFirst::First);
	assert_eq!(DefaultNotFirst::from_primitive(0), DefaultNotFirst::Default);
	assert_eq!(DefaultNotFirst::from_primitive(3), DefaultNotFirst::Third);
	assert_eq!(DefaultNotFirst::from_primitive(2), DefaultNotFirst::Default);
}

// Test with expressions as discriminants
#[derive(ConstFromPrimitive, Default, Debug, PartialEq)]
#[repr(i32)]
enum ExprEnum {
	#[default]
	Default = 0,
	Value = 1 + 1,
	Shifted = 1 << 4,
}

#[test]
fn test_expression_discriminants() {
	assert_eq!(ExprEnum::from_primitive(2), ExprEnum::Value);
	assert_eq!(ExprEnum::from_primitive(16), ExprEnum::Shifted);
	assert_eq!(ExprEnum::from_primitive(0), ExprEnum::Default);
	assert_eq!(ExprEnum::from_primitive(1), ExprEnum::Default);
}

// Test single variant enum
#[derive(ConstFromPrimitive, Default, Debug, PartialEq)]
#[repr(i32)]
enum SingleVariant {
	#[default]
	Only = 42,
}

#[test]
fn test_single_variant() {
	assert_eq!(SingleVariant::from_primitive(42), SingleVariant::Only);
	assert_eq!(SingleVariant::from_primitive(0), SingleVariant::Only);
	assert_eq!(SingleVariant::from_primitive(-42), SingleVariant::Only);
}

// Additional tests for edge cases
#[derive(ConstFromPrimitive, Default, Debug, PartialEq)]
#[repr(u16)]
enum U16Enum {
	#[default]
	Default = 0,
	Middle = 32768,
	Max = 65535,
}

#[test]
fn test_u16_boundaries() {
	assert_eq!(U16Enum::from_primitive(0), U16Enum::Default);
	assert_eq!(U16Enum::from_primitive(32768), U16Enum::Middle);
	assert_eq!(U16Enum::from_primitive(65535), U16Enum::Max);
	assert_eq!(U16Enum::from_primitive(1), U16Enum::Default);
	assert_eq!(U16Enum::from_primitive(65534), U16Enum::Default);
}

#[derive(ConstFromPrimitive, Default, Debug, PartialEq)]
#[repr(i8)]
enum I8Enum {
	#[default]
	Default = 0,
	Min = i8::MIN,
	Max = i8::MAX,
}

#[test]
fn test_i8_boundaries() {
	assert_eq!(I8Enum::from_primitive(0), I8Enum::Default);
	assert_eq!(I8Enum::from_primitive(i8::MIN), I8Enum::Min);
	assert_eq!(I8Enum::from_primitive(i8::MAX), I8Enum::Max);
	assert_eq!(I8Enum::from_primitive(1), I8Enum::Default);
}

// Test with many variants to ensure no stack overflow or performance issues
#[derive(ConstFromPrimitive, Default, Debug, PartialEq)]
#[repr(i32)]
enum ManyVariants {
	#[default]
	Default = 0,
	V001 = 1,
	V002 = 2,
	V003 = 3,
	V004 = 4,
	V005 = 5,
	V006 = 6,
	V007 = 7,
	V008 = 8,
	V009 = 9,
	V010 = 10,
	V011 = 11,
	V012 = 12,
	V013 = 13,
	V014 = 14,
	V015 = 15,
	V016 = 16,
	V017 = 17,
	V018 = 18,
	V019 = 19,
	V020 = 20,
}

#[test]
fn test_many_variants() {
	assert_eq!(ManyVariants::from_primitive(0), ManyVariants::Default);
	assert_eq!(ManyVariants::from_primitive(1), ManyVariants::V001);
	assert_eq!(ManyVariants::from_primitive(10), ManyVariants::V010);
	assert_eq!(ManyVariants::from_primitive(20), ManyVariants::V020);
	assert_eq!(ManyVariants::from_primitive(21), ManyVariants::Default);
	assert_eq!(ManyVariants::from_primitive(-1), ManyVariants::Default);
}

// Test that the generated code works in a const fn context
#[derive(ConstFromPrimitive, Default, Debug, PartialEq)]
#[repr(i32)]
enum ConstFnEnum {
	#[default]
	Default = 0,
	A = 1,
	B = 2,
}

const fn get_value(x: i32) -> ConstFnEnum {
	ConstFnEnum::from_primitive(x)
}

#[test]
fn test_in_const_fn() {
	const VAL_A: ConstFnEnum = get_value(1);
	const VAL_B: ConstFnEnum = get_value(2);
	const VAL_DEFAULT: ConstFnEnum = get_value(99);

	assert_eq!(VAL_A, ConstFnEnum::A);
	assert_eq!(VAL_B, ConstFnEnum::B);
	assert_eq!(VAL_DEFAULT, ConstFnEnum::Default);
}
