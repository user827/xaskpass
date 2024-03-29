# Xaskpass
[![AUR version](https://img.shields.io/aur/version/xaskpass)](https://aur.archlinux.org/packages/xaskpass/)
[![Crate](https://img.shields.io/crates/v/xaskpass.svg)](https://crates.io/crates/xaskpass)
![Minimum rustc version](https://img.shields.io/badge/rustc-1.74+-lightgray)

Xaskpass is a lightweight passphrase dialog for X11 with extensive configuration
options that is implemented without relying on heavy GUI libraries. It aims to
be a successor to the similar but now old [x11-ssh-askpass] by preserving
its fast startup time while modernizing some features such as fonts. It also tries
to make sure the password stays in the memory for the shortest time.

[x11-ssh-askpass]: https://archlinux.org/packages/community/x86_64/x11-ssh-askpass/

<p align="center">
<img src="res/circle.png">
</p>

Classic indicator | Strings/Asterisk | Strings/Disco
:-------:|:-------:|:-------:
![](res/classic.png) | ![](res/asterisk.png) | ![](res/disco.png)

## Installation and building
In Arch Linux the easiest way to install is to use the [aur package](https://aur.archlinux.org/packages/xaskpass).

If the C libraries are already installed, cargo install can be used to install
in ~/.cargo/bin/xaskpass:
```sh
cargo install xaskpass
```

To build from the repository, use:
```sh
cargo build --release --locked
```

Make sure `rustc` is 1.74+ ([reason](https://docs.rs/clap/latest/clap/)) and you have the following C libraries installed:

* libxcb >= [1.12](https://crates.io/crates/x11rb/0.11.1)
* libxkbcommon
* libxkbcommon-x11
* clang >= [5.0](https://rust-lang.github.io/rust-bindgen/requirements.html#clang)
* cairo >= [1.14](https://crates.io/crates/cairo-rs/0.17.0)
* pango >= [1.50](https://github.com/user827/xaskpass/commit/c328d87ac9207bd074f457d117c26f79930a9137)

For example in Arch Linux you can run:
```sh
pacman -S libxkbcommon libxkbcommon-x11 libxcb pango cairo clang
```

## Setup
To make `ssh` or `sudo` use `xaskpass` set
`SSH_ASKPASS=/path/to/xaskpass` or `SUDO_ASKPASS` (and use `sudo -A`) respectively.

## Configuration

Xaskpass firsts tries to read configuration from `$XDG_CONFIG_HOME/xaskpass/xaskpass.toml`. If not found,
`$XDG_CONFIG_DIRS/xaskpass/xaskpass.toml` is tried.
A default configuration file with comments can be found [here](xaskpass.default.toml).

To make the startup time faster, for example, the font file used can be specified with
```toml
[dialog]
font_file = '/path/to/fonts/TTF/DejaVuSansMono.ttf'
```

## More help

See `xaskpass --help` and the comments in [the default configuration
file](xaskpass.default.toml).

## Development

You can create directory `pregen` to speed up `build.rs` by letting it save the
generated bindings there.

## License

Xaskpass is released under the [Apache License, Version 2.0](LICENCE).
