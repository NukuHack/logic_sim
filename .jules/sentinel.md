## 2026-09-04 - Unvalidated Chip Names in Save Files Path Traversal
**Vulnerability:** `Loader::load_chip_library` loaded custom chip names directly from `ProjectDescription.json`'s `all_custom_chip_names` array without filename validation, allowing path traversal (e.g. `../../etc/passwd`) when opening untrusted or modified project files.
**Learning:** External or persisted JSON data cannot be assumed safe even if created by the same app; filesystem operations must validate string values against OS path rules (`valid_file_name`) before joining paths.
**Prevention:** Validate all file/project names at the API boundaries of `Loader` and `Saver` using `valid_file_name` prior to constructing path buffers.
