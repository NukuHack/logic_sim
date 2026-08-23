//! Thin entry point for the integrated Digital Logic Sim app: project
//! picker -> chip viewer in one window. All the actual frontend logic
//! lives in the `logic_sim::viewer` module so it stays headless-testable
//! alongside the rest of the library.

fn main() {
	if let Err(e) = logic_sim::viewer::run() {
		eprintln!("event loop error: {e}");
		std::process::exit(1);
	}
}
