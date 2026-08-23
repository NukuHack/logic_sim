//! A loaded project: its metadata (`ProjectDescription`) plus the chip library built from it
//! (custom chips loaded from disk + all builtins). This mirrors only the persistence-relevant slice
//! of `DLS.Game.Project` -- the original also carries a large amount of live editor/simulation state
//! (undo stacks, camera, the currently-viewed chip, audio, ...) that belongs with the editor/sim
//! integration this port doesn't include yet, not with save/load.

use crate::description::ChipDescription;
use crate::json::ProjectDescription;
use crate::ChipLibrary;

pub struct Project {
	pub description: ProjectDescription,
	pub chip_library: ChipLibrary,
}

impl Project {
	pub fn new(description: ProjectDescription, chip_library: ChipLibrary) -> Self {
		Self { description, chip_library }
	}

	/// Adds (or replaces) a custom chip in this project's in-memory library
	/// and, if it wasn't already listed, appends it to `AllCustomChipNames`.
	/// Does not touch disk -- pair with `Saver::save_chip` /
	/// `Saver::save_project_description` to persist the change.
	pub fn add_or_update_custom_chip(&mut self, desc: ChipDescription) {
		let already_known = self.description.all_custom_chip_names.iter().any(|n| n.eq_ignore_ascii_case(&desc.name));
		if !already_known {
			self.description.all_custom_chip_names.push(desc.name.clone());
		}
		self.chip_library.add(desc);
	}
}
