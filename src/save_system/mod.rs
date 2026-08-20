//! Save/load system: project + app-settings persistence, ported from
//! `DLS.SaveSystem` (`Saver`, `Loader`, `SavePaths`, `SaveUtils`,
//! `UpgradeHelper`) plus the persistence-relevant parts of `DLS.Game.Main`
//! and `DLS.Game.BuiltinCollectionCreator`.
//!
//! Layout:
//! - [`paths`] -- where things live on disk (`SavePaths`)
//! - [`version`] -- the `major.minor.patch` version type used for
//!   compatibility checks (`Version`, `DLS_VERSION`, ...)
//! - [`util`] -- filename validation / unique-naming / directory-copy helpers
//! - [`timestamp`] -- ISO-8601 timestamp formatting for save metadata
//! - [`defaults`] -- default starred list / chip collections for new projects
//! - [`project`] -- the in-memory `Project` (description + chip library)
//! - [`saver`] / [`loader`] -- read/write project & settings files
//! - [`orchestration`] -- create-or-load-project + version compatibility

mod defaults;
mod loader;
mod orchestration;
mod paths;
mod project;
mod saver;
mod timestamp;
mod util;
mod version;

#[cfg(test)]
pub(crate) mod test_util;

pub use defaults::{default_chip_collections, default_starred_list};
pub use loader::Loader;
pub use orchestration::{can_open_project, create_or_load_project, create_project};
pub use paths::{SavePaths, PROJECT_FILE_NAME};
pub use project::Project;
pub use saver::Saver;
pub use util::{copy_directory, ensure_unique_directory_name, ensure_unique_file_name, name_contains_forbidden_char, valid_file_name};
pub use version::{Version, VersionParseError, DLS_VERSION, DLS_VERSION_EARLIEST_COMPATIBLE};
