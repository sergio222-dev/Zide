# Development

Building and testing Zide, and the environment variables that help debug it.

[Back to README](../README.md)

## Tests and hooks

```sh
cargo test --workspace
git config core.hooksPath .githooks   # once: formatting on commit, CI's checks on push
```

Releasing is described in [docs/releasing.md](releasing.md).

## Frame rate (debug)

`ZIDE_FPS=1 zide` shows an overlay with the rate of the burst of frames in progress (a scroll
glide, for instance), the frames of the last second, the longest wait between two frames of a
burst and the time spent building the view. The window only redraws when
something changes, so read it while scrolling or holding a movement key.

## Update notification (debug)

`ZIDE_FAKE_VERSION=0.0.1 zide` makes the update check believe it runs that version, so the
"Update available" notification shows up. The Update button installs the real latest release.

[Back to README](../README.md)
