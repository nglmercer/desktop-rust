# Platform and release gaps

The ordered migration worklist is [`ROADMAP.md`](ROADMAP.md). This file tracks
only cross-platform and release gaps that should not block Linux migration slices.

## Core workflows

- [ ] Complete publication push support, including Git LFS, automatic upstream
      configuration, durable phase records, and explicit recovery for uncertain mutations.
- [ ] Reconcile older receipts that lack enough verified identity to resume.
- [ ] Support selected-account credentials consistently for clone, worktrees,
      and submodules.
- [ ] Complete remaining authentication, account-import, Git, and GitHub parity
      before deleting corresponding legacy code.

## Platforms

- [ ] Validate Windows and macOS filesystem identity, safe configuration,
      credentials, protocol registration, editors, terminals, and shell behavior.
- [ ] Validate the Tao/Wry shell on real graphical systems; Linux evidence is
      mostly under Xvfb.
- [ ] Validate the headless/Termux path on a real low-resource device.

## Release

- [ ] Produce signed/notarized installers and a signed update channel controlled
      by this fork.
- [ ] Complete license and notice inventory validation for npm, Rust, and
      bundled native components.
- [ ] Exercise a relocated production package outside the checkout on every
      supported desktop platform.
