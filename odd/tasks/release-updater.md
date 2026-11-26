# Installable tarball + self-updater with versioning
Objective: ship Zide as a `.tar.gz` with the binary, versioned with SemVer, and update itself from GitHub Releases verifying a minisign signature.
Branch: main. Delivery: commit per task. Runner: `cargo test --workspace`.

## Specs
S1. "un instalable con updater? versioning" — SemVer in the workspace, tags `vX.Y.Z`, `zide --version`.
S2. "no me gusta usar appimage, prefiero un .tar.gz con el binario" — release asset `zide-<version>-x86_64-unknown-linux-gnu.tar.gz` holding the `zide` binary.
S3. "firma minisign" — every asset has a `.minisig`; Zide embeds the public key and refuses anything that does not verify. No key configured = updater disabled (fail closed).
S4. Repo: https://github.com/sergio222-dev/Zide (user-owned). Nothing is pushed, tagged or released without the user's explicit say-so. Assumed public (no token handling).
S5. Linux x86_64 only for now (assumption: the user did not answer about macOS/Windows).
S6. The UI never blocks on the network: check and install run in the background; installing is always an explicit user action ("Update Zide").

## Tasks
- [x] T1 (S1,S2) version 0.1.0, binary named `zide`, `--version`. ce99fc3
- [x] T2 (S3,S6) crate zide-update: release parsing, version compare, signature check, tar.gz extraction, atomic replace; tests with an injected fetcher and a test keypair. 44dd7c0
- [x] T3 (S6) UI: startup check, "Update Zide" action, status line. c2f7a71
- [x] T4 (S2,S3) GitHub Actions: CI (fmt, clippy, test) and release on tag (build, tar.gz, sign, publish). 962f35d (unrun: needs a tag + secrets)
- [x] T5 docs/releasing.md: generate keys, secrets, cut a release, install. 962f35d

## Log
L1 (2026-11-05) "se puede hacer un instalable con updater? versioning"
L2 "agregue el repo, no me gusta usar appimage, prefiero un .tar.gz con el binario"
L3 "firma missing" (read as: minisign)
