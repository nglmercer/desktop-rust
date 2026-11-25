# Remaining migration work

This file is a short supplement to [`ROADMAP.md`](ROADMAP.md). The roadmap is
the ordered Linux worklist; items here are broader platform and release gaps.
They are open work, not completed features.

## Core and CLI

- [ ] Complete publication push support, including Git LFS, automatic upstream
      configuration, durable phase records and explicit recovery for uncertain
      mutations.
- [ ] Add reconciliation for older receipts that do not contain enough verified
      identity to resume.
- [ ] Support selected-account credentials consistently for clone, worktrees
      and submodules.
- [ ] Complete authentication, account import and remaining Git/GitHub workflow
      parity, then remove the corresponding legacy code only after replacement
      evidence exists.

## Environment and platforms

- [ ] Validate Windows and macOS filesystem identity, safe configuration,
      credentials, protocol registration, editors, terminals and shell behavior.
- [ ] Replace conservative system-configuration requirements with portable,
      verified discovery where the security contract allows it.
- [ ] Validate the existing Tao/Wry desktop shell on real graphical Windows and
      macOS systems; Linux currently has the strongest evidence, mostly under Xvfb.
- [ ] Validate the headless/Termux path without GUI dependencies on a real
      low-resource device.

## Packaging and release

- [ ] Produce signed/notarized installers and a signed update channel controlled
      by this fork.
- [ ] Complete license/notice inventory validation for npm, Rust and bundled
      native components.
- [ ] Exercise a relocated production package outside the checkout on every
      supported desktop platform.
