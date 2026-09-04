## 2025-05-18 - Stack allocation and reference borrowing in simulator hot loop
**Learning:** `process_cached_chip` in `src/sim.rs` gets called on every simulation frame for cached combinational subchips. Allocating a `Vec` for output scratch space and cloning `Arc<str>` names per cached evaluation created unnecessary heap allocation and atomic refcount thrashing in the simulation hot loop.
**Action:** Use fixed-size stack arrays (`[0u32; 32]`) for common output pin counts with fallback to `Vec`, and borrow `&str` directly from arena storage instead of `Arc::clone`.
