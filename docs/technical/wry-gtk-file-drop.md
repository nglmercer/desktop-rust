# Wry GTK file drop investigation

The repository drop handler currently relies on Electron's
`webUtils.getPathForFile`; the Vite renderer shim returns an empty path. A
replacement for the Rust host remains necessary.

On Linux with Wry 0.57.2, an experiment installed
`WebViewBuilder::with_drag_drop_handler` and returned `false` from its callback.
In an isolated Xvfb session with a private home and mount namespace, physical
drags were simulated from Dolphin using `xdotool`:

- Dropping a directory onto the page delivered a native drop and its path, but
  no DOM `drop` event was observed.
- Dropping a file onto an `<input type="file">` delivered a native drop, but
  neither a DOM `drop` nor a `change` event with a `FileList` was observed.

The GTK implementation calls `drop_finish(true)` before invoking the handler,
so returning `false` alone does not establish that the normal HTML handling is
preserved. The candidate was removed. This experiment did not compare a build
without the handler or test internal drags, and does not establish behavior on
macOS or Windows.

Any replacement also needs to account for coordinate scaling and layout
changes when identifying a drop target. Wry's GTK URI decoding is lossy for
non-UTF-8 names: an invalid name can collide with a different name containing
U+FFFD. Rejecting U+FFFD avoids that ambiguity but also rejects legitimate
names containing that character.
