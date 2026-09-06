use logic_sim_macros::ConstFromPrimitive;

#[derive(ConstFromPrimitive, Default)]
#[repr(i32)]
enum NoDiscriminant {
    #[default]
    Default = 0,
    NoValue, // No explicit discriminant
}

fn main() {}