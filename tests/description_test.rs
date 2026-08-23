//! `WireDescription` constructor invariants: a plain wire attaches both
//! ends directly to pins, and a tapped-source wire records the tap
//! (segment + cached attachment point) while keeping the *real* signal
//! source address -- the on-disk conventions every other layer relies on.

use logic_sim::description::{PinAddress, WireConnectionType, WireDescription};
use logic_sim::Vec2;

#[test]
fn new_wire_attaches_both_ends_directly_to_pins() {
	let wire = WireDescription::new(PinAddress::new(1, 0), PinAddress::new(2, 0));
	assert_eq!(wire.connection_type, WireConnectionType::ToPins);
	assert_eq!(wire.connected_wire_index, -1);
	assert_eq!(wire.connected_wire_segment_index, -1);
	assert!(wire.points.is_empty());
}

#[test]
fn new_tapped_source_records_the_tap_and_keeps_the_real_source_address() {
	let tapped_source = PinAddress::new(1, 0);
	let target = PinAddress::new(3, 1);
	let tap_point = Vec2::new(2.5, 1.5);

	let wire = WireDescription::new_tapped_source(tapped_source, target, 0, 1, tap_point);

	assert_eq!(wire.connection_type, WireConnectionType::ToWireSource);
	assert_eq!(wire.source_pin_address, tapped_source);
	assert_eq!(wire.target_pin_address, target);
	assert_eq!(wire.connected_wire_index, 0);
	assert_eq!(wire.connected_wire_segment_index, 1);
	assert_eq!(wire.cached_source_point, tap_point);
	assert!(wire.points.is_empty());
}
