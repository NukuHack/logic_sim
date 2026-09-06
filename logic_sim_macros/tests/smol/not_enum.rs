use logic_sim_macros::ConstFromPrimitive;

#[derive(ConstFromPrimitive, Default)]
struct NotAnEnum {
    x: i32,
}

fn main() {}