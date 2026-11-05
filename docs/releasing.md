# Releasing and installing Zide

## Install

Download `zide-<version>-x86_64-unknown-linux-gnu.tar.gz` from the
[releases page](https://github.com/sergio222-dev/Zide/releases), then:

```sh
tar -xzf zide-*-x86_64-unknown-linux-gnu.tar.gz
install -Dm755 zide-*/zide ~/.local/bin/zide
zide --version
```

Only Linux x86_64 is published. Zide updates itself: it checks for a newer release at
startup and says so in the editor's status line; run **Update Zide** (double-Shift, Actions tab)
to install it. The new version runs on the next start. The updater needs write permission on
the folder holding the `zide` binary (use `~/.local/bin`, not `/usr/bin`).

An update is installed only if the archive's [minisign](https://jedisct1.github.io/minisign/)
signature verifies against the public key compiled into the running binary
(`crates/update/zide.pub`). A build without a key never updates.

## One-time setup

```sh
mkdir -p ~/.minisign
minisign -G -p crates/update/zide.pub -s ~/.minisign/zide.key
```

1. Commit `crates/update/zide.pub`. Releases built before it exists cannot update anyone.
2. In the GitHub repository, *Settings → Secrets and variables → Actions*, add:
   - `MINISIGN_SECRET_KEY`: the contents of `~/.minisign/zide.key`
   - `MINISIGN_PASSWORD`: its password (leave the key password-less and set this to an empty
     value if you prefer)
3. Keep `~/.minisign/zide.key` safe. Losing it means users must reinstall by hand; a leaked one
   lets anyone ship updates.

## Cut a release

1. Bump `version` in the root `Cargo.toml` ([SemVer](https://semver.org)) and commit.
2. `git tag vX.Y.Z && git push origin main vX.Y.Z`
3. The `Release` workflow checks the tag equals the crate version, tests, builds, packs
   `zide-X.Y.Z-x86_64-unknown-linux-gnu.tar.gz`, signs it (`.minisig`) and publishes both.

Pre-releases and drafts are ignored by the updater (it asks GitHub for the *latest* release).
