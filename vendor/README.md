# Vendored crates

## winit 0.30.13

A copy of [winit 0.30.13](https://crates.io/crates/winit/0.30.13) from crates.io with one change, wired in through `[patch.crates-io]` in `Cargo.toml`.

**Problem.** On Windows, with two monitors at different display scales (for example 100% and 150%), the editor window could not be moved onto the other monitor, by dragging or with Win+Shift+Arrow: it stopped at the monitor edge.

**Cause.** When the window reaches a monitor with another scale, Windows sends `WM_DPICHANGED` with a suggested rectangle on the new monitor. winit's handler decided which monitor that was with `MonitorFromWindow`, which at that moment still returns the monitor being left. Its "nudge onto the new-DPI monitor" loop then pushed the window back onto the old one.

**Change.** `src/platform_impl/windows/event_loop.rs`, in the `WM_DPICHANGED` handler: the new-DPI monitor is taken from the suggested rectangle with `MonitorFromRect`, falling back to `MonitorFromWindow` if the rectangle is on no monitor. Search for `red_text_editor patch`.

The same approach is proposed upstream in [rust-windowing/winit#4704](https://github.com/rust-windowing/winit/pull/4704).

**Removing it.** Once eframe depends on a winit release with this fixed, delete `vendor/winit`, this file, and the `[patch.crates-io]` section in `Cargo.toml`, then run `cargo update -p winit`.
