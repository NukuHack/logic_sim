use logic_sim_macros::ConstFromPrimitive;

#[derive(ConstFromPrimitive)]
#[repr(i32)]
enum NoDefault {
    A = 1,
    B = 2,
}

fn main() {}