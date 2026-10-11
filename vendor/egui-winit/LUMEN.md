# egui-winit, patched for Lumen

This is egui-winit 0.36.2 from crates.io (MIT OR Apache-2.0, see the license
files) with one change: the Menu key (`NamedKey::ContextMenu`, next to the
right Ctrl) used to be dropped because egui has no key for it. It now arrives
as Shift+F10, its keyboard equivalent on Windows and GTK, so Lumen opens its
right-click menu with either. Search `src/lib.rs` for "Lumen patch".

Lumen's `Cargo.toml` uses it through `[patch.crates-io]`. When eframe moves to
a new egui-winit, Cargo ignores this copy with only a warning; Lumen's test
`the_patched_egui_winit_is_the_one_built` then fails. To redo it:

1. Copy the new egui-winit from `~/.cargo/registry/src/*/egui-winit-<version>`
   over this folder (keep this file and the license files).
2. Re-apply the "Lumen patch" block in `on_keyboard_input` and `menu_key_event`.
3. Run `cargo test` and
   `cargo test --manifest-path vendor/egui-winit/Cargo.toml --lib lumen_patch`.
