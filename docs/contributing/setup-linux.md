# Setting Up Development Dependencies on Linux

You will need to install these tools on your machine:

 - Node.js (includes npm)
 - Python 3
 - Webview dependencies

## Node.js

The NodeJS project has instructions for installing across a variety of
distributions and package managers.

Find your distribution on [this list](https://nodejs.org/en/download/package-manager/)
and follow the instructions to install the version you require.

Ensure that you also choose the option for building native Node modules, as
those are used in some dependencies used in GitHub Desktop.

## npm

Use the npm version bundled with Node.js. Some distributions provide npm as a
separate package; verify both `node -v` and `npm -v` are available on your `PATH`.

Desktop uses `package-lock.json` files to pin dependencies across machines.
See [working with packages](./working-with-packages.md) for installation,
dependency updates, and script commands.

## Python 3

Refer to your distributions package manager to obtain the latest version of the
Python 3 series.

## Webview dependencies

The Rust desktop host renders with WebKitGTK. These are required to build and
run GitHub Desktop locally:

 - `libwebkit2gtk-4.1` and `libgtk-3` (development headers to build)
 - `libdbus-1`, and `libxdo` for the native shell
 - `libsecret-1.so.0` for reading and writing credentials

Where to find these will vary based on your distribution, but below are some
examples of distributions we've tested.

### Fedora 26 and later

```shellsession
$ sudo dnf install -y libsecret-devel webkit2gtk4.1-devel gtk3-devel dbus-devel libxdo-devel
```

### Ubuntu 14.04 and later

```shellsession
$ sudo apt install libsecret-1-dev libwebkit2gtk-4.1-dev libgtk-3-dev libdbus-1-dev libxdo-dev
```

## Back to setup

Once you've installed the necessary dependencies, head back to the [setup page](https://github.com/desktop/desktop/blob/development/docs/contributing/setup.md) to finish getting set up.
