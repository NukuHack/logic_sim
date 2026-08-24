//! Draft state for an in-progress wire placement: the pending endpoint
//! pair/bend list that lives on [`crate::viewer::state::ViewerState`] while
//! the player is drawing a wire, kept apart from the rest of the viewer's
//! working state because it carries the pin-address/tap bookkeeping the
//! canvas interaction (and its preview) needs.

use crate::description::PinAddress;
use crate::structs::Vec2;
use crate::PinBitCount;

/// One endpoint of an in-progress wire placement (`ViewerState::pending_wire`),
/// fixed at the moment the wire is started -- either a real pin (a
/// subchip's own, or one of the current chip's own boundary dev-pins) or
/// a tap point along an existing wire's line.
#[derive(Debug, Clone, Copy)]
pub(crate) enum PendingWireEnd {
	Pin {
		owner_id: i32,
		pin_id: i32,
		is_source: bool,
		position: Vec2,
	},
	/// Tapping onto a wire always plays the *source* role for the new
	/// branch wire (see `try_start_pending_wire`'s doc comment) -- the
	/// tapped wire's own real source pin, needed to build the eventual
	/// `WireDescription::new_tapped_source`, travels along here rather
	/// than being re-looked-up at completion time.
	WireTap {
		wire_index: usize,
		segment_index: i32,
		point: Vec2,
		source_pin_address: PinAddress,
	},
}

impl PendingWireEnd {
	pub(crate) fn is_source(&self) -> bool {
		match self {
			PendingWireEnd::Pin { is_source, .. } => *is_source,
			PendingWireEnd::WireTap { .. } => true,
		}
	}

	pub(crate) fn position(&self) -> Vec2 {
		match self {
			PendingWireEnd::Pin { position, .. } => *position,
			PendingWireEnd::WireTap { point, .. } => *point,
		}
	}
}

/// State for an in-progress wire placement: the endpoint it started
/// from, plus any bend ("turn") points the player has since clicked on
/// empty canvas space, in click order -- becomes the finished
/// `WireDescription::points` once the wire is completed at a second,
/// opposite-role endpoint (reversed first if that second endpoint turns
/// out to be the wire's real *source*, since `points` always runs
/// source-to-target). `None` on [`crate::viewer::state::ViewerState`] whenever no
/// wire is being placed.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(crate) struct PendingWire {
	pub(crate) start: PendingWireEnd,
	pub(crate) bend_points: Vec<Vec2>,
	pub(crate) bit_count: PinBitCount,
}
