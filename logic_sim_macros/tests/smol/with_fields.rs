use logic_sim_macros::ConstFromPrimitive;

#[derive(ConstFromPrimitive, Default)]
#[repr(i32)]
enum WithFields {
    #[default]
    Default = 0,
    Tuple(i32),
    Struct { x: i32 },
}

fn main() {}