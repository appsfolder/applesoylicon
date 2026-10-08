# applesoylicon

A tiny, fast Matrix-rain terminal screensaver with a hidden message and no
surprise system changes. Written in dependency-free Rust for Linux terminals.

```console
$ cargo build --release
$ ./target/release/applesoylicon --palette soy
```

Exit with `q`, `Esc`, or `Ctrl-C`. While it runs, press `p` to cycle palettes,
`+`/`-` to change animation speed, or `d` to toggle dense mode.

## Options

```text
--palette <green|amber|ice|soy>
--fps <5..60>
--density <0.05..1.0>
--message <ASCII text, up to 40 characters>
--seed <integer>
```

## Install from source

```console
$ cargo install --path .
```

## Arch Linux / AUR

The `packaging/aur` directory contains a release `PKGBUILD`. Before submitting
the first AUR release:

1. Push this repository to `https://github.com/appsfolder/applesoylicon`.
2. Create and push tag `v1.0.0`.
3. Create a GitHub release for the tag and attach the provided
   `applesoylicon-1.0.0.tar.gz` release archive without modifying it.
4. Run `makepkg --printsrcinfo > .SRCINFO`, then test with `makepkg -sic` and
   `namcap` if available.
5. Clone the empty AUR repository and copy in `PKGBUILD` and `.SRCINFO`.

The AUR package name `applesoylicon` was chosen to match the executable.

## License

MIT
