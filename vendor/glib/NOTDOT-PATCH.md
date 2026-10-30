# GLib 0.18.5 safety backport

Source: crates.io glib 0.18.5, unchanged except `src/variant_iter.rs` (make the FFI output pointer mutable and pass `&mut p`). Original licenses are retained.

Backport of https://github.com/gtk-rs/gtk-rs-core/pull/1343 for RUSTSEC-2024-0429. GTK3/Tauri pins the 0.18 major, so upgrading just GLib to 0.20 is not ABI/API compatible. Remove this override when the platform stack releases a compatible fix or moves past GTK3. The advisory version range still matches 0.18.5; this is a documented local backport, not an ignored risk. No version spoofing or audit suppression is used.

Validation: run optimized GLib variant tests on Linux. Cache/build artifacts remain ignored.
