# Experimental Vite Electron renderer

The production and development Electron builds still use Webpack for the
renderer. The optional `npm run compile:renderer:vite` command builds the
experimental Linux renderer into `node_modules/.cache/desktop-renderer-vite` without changing
those builds. It produces HTML, CSS, JavaScript, native addons and the Koffi
runtime in relocatable paths. The installed `fs-admin` and `keytar` packages
must contain their compiled native addons. For testing an installation whose
native addons are elsewhere, set `DESKTOP_RENDERER_FS_ADMIN_NODE` and
`DESKTOP_RENDERER_KEYTAR_NODE` to their absolute paths.

A Linux integration probe builds the current Electron main process and the Vite
renderer, overlays them onto a temporary copy of an existing installation, and
starts that copy with an isolated profile. It creates a local repository,
commits a file through the UI, verifies the commit with Git, and loads the FFI
chunk from the relocated app. This exercises the real native addons and needs
an installed app with compatible assets and native modules, Git, and an X
server. For example:

```sh
DESKTOP_VITE_PROBE_APP=/opt/github-desktop/resources/app \
  xvfb-run -a npm run test:renderer:vite
```

The base directory is required and must be absolute. The probe does not test a
self-contained packaged build: it uses native dependencies and other assets
from that installation. It uses a temporary profile, disables inherited Git
configuration and hooks, and disconnects the session D-Bus to avoid the user's
keyring. It does not observe renderer errors emitted before attaching to the
window. The temporary profile is retained if Electron's process group cannot
be stopped within the cleanup timeout.

Before replacing Webpack, the renderer still needs Windows and macOS native
addon support, a development rebuild and reload workflow, production packaging
and source-map validation, and cross-platform end-to-end coverage. Vite's
transpilation also does not replace the full TypeScript type check currently
performed by the Webpack build. The experimental command deliberately rejects
non-Linux builds until those platform-specific dependencies are handled.
