# Building and Packaging Desktop

This document outlines how Desktop is currently packaged for all platforms -
there are some parts in common but then each platform has it's own quirks.

## Vite

GitHub Desktop uses Vite to transpile and bundle its application resources.
The configuration files are found at the repository root:

 - `vite.main.config.ts` - `main.js`, the Electron main process
 - `vite.renderer.config.ts` - `renderer.js`, `renderer.css` and `index.html`
   for the Electron renderer
 - `vite.crash.config.ts` - `crash.js`, the UI shown when the app crashed
 - `vite.highlighter.config.ts` - `highlighter.js`, the syntax-highlighting
   worker
 - `vite.cli.config.ts` - `cli.js`, the `github` command line interface

Vite also handles these steps:

 - placeholders in source for platform-specific values are replaced, using
   `app/app-info.ts` and the values of the platform the build is being performed
   on
 - SCSS stylesheets under `app/styles/` are transpiled to CSS and emitted
 - source maps are generated to help with correlating runtime errors to the
   corresponding TypeScript source

Vite does not type-check, so `compile:dev` and `compile:prod` run
`npm run check:types` before the renderer build.

The output is stored in the `out` directory, which is ignored in version
control.

The renderer includes the Copilot SDK's native Koffi dependency. Only the
`@koromix/koffi-<platform>-<architecture>` package matching the build target is
bundled, even if cross-compilation setup installed optional packages for other
platforms. The architecture comes from `getDistArchitecture()` (including its
`npm_config_arch` override), not necessarily the build machine's architecture.
The target package must be installed; a missing package fails the build.
`npm run test:script` verifies that multiple installed Koffi packages produce only
the target's `koffi.node`, without filename collisions.

## `app/package.json`

The `version` attribute in `app/package.json` is the canonical reference for
the version number you see in the **About** view of GitHub Desktop.

## `changelog.json`

This file is used to track user-facing changes associated with each Desktop
release. This can include new features, bug fixes, improvements or even removed
features.

## `script/build.ts`

After the Vite build the next component in the pipeline is  `script/build.ts` which
handles moving app resources into place and launching `electron-packager`, which
merges the application resources and Electron runtime for the current
platform into an executable program.

This script is responsible for:

 - merging additional static resources into the output directory
 - generating a license bundle from the projects that Desktop uses, which are
   accessible from the **About GitHub Desktop** dialog
 - generating license metadata from the `choosealicense.com` source repository,
   so the user can add a suitable license to their new repository when creating
   a repository in Desktop

`electron-packager` is also responsible for code-signing on macOS at this stage,
and if you do not have this configured you will likely see an error like this:

```
Packaging app for platform darwin x64 using electron v2.0.9
WARNING: Code sign failed; please retry manually. Error: No identity found for signing.
```

This is fine for development purposes, but code-signing is highly recommended
when distributing Desktop to end users.

## `script/package.ts`

After the build is the final step is to package the application for distribution
to end users, and this is where much of the platform-specific variations occur.

### macOS

macOS does not need any additional work, as the previous step generated an
application bundle and performed the required code-signing.

The generated app is compressed into a `.zip` archive, reducing the download
size by ~60%.

### Windows

Desktop uses `electron-winstaller` to generate two installers:

 - a [Squirrel](https://github.com/Squirrel/Squirrel.Windows)-based `.exe` that
   does not require elevated permissions to install
 - an installer based on [Windows Installer](https://docs.microsoft.com/en-us/windows/desktop/msi/windows-installer-portal)
   that supports being installed by administrators. Please note that this is a
   partial solution and this still leverages `%LOCALAPPDATA%` when each users
   runs the app for the first time. See [#1086](https://github.com/desktop/desktop/issues/1086)
   for an ongoing discussion about alternatives.

Other things to note about the Windows packaging process:

 - `electron-winstaller` is responsible for signing these installers
 - Squirrel supports generating delta packages (representing the difference
   between this version and the previous version) to avoid downloading bytes
   that haven't changed. This has been enabled for Desktop, and requires
   downloading the previous version from Central to generate the delta package.

### Linux

Refer to the [`shiftkey/desktop`](https://github.com/shiftkey/desktop) fork
for packaging details about Linux.

## `script/publish.ts`

This script uploads the packaging artifacts to our S3 bucket, which is then
verified before the build is made available to users.
