## What changed

Describe the user-visible behavior and the reason for the change. Link the
issue (`Fixes #123`).

## Verification

- [ ] `cargo fmt --all --check`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo test --workspace`
- [ ] `cargo deny check`
- [ ] If `Cargo.lock` changed: `python scripts/slint/crate-notices.py --check`
      with the `cargo tree` lists described in the script (the crate inventory
      in `build/THIRD-PARTY-NOTICES.txt` matches `Cargo.lock`)
- [ ] No secrets, build output, game archives, or user data were added
- [ ] Third-party licenses and notices were preserved

## Security and compatibility

Describe changes to IPC, downloads, signatures, file paths, process launches,
updates, or supported Windows versions. Write `Not applicable` when none apply.

---

By submitting this pull request, I agree that my contribution is licensed
under the project license (GPL-3.0-only) and that I have the right to license
it so.
