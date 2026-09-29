---
name: system-packages
description: Install a Debian package in your Computer with sudo pagis-apt install, and the rules that wrapper applies.
---

# System packages

Your Computer runs Debian. `apt-get` needs root, and `pagis-apt` is the only
command you can run as root.

## Install a package

```bash
sudo pagis-apt install ripgrep jq
```

The wrapper refreshes the package lists when they are absent or more than one
day old, then it installs. The first install in a new Computer takes longer,
because the image ships with no lists.

## The rules the wrapper applies

- `install` is the only subcommand. There is no `remove`, `upgrade` or
  `search`.
- Each argument must be a Debian binary package name: lower-case letters,
  digits, `.`, `+` and `-`, and never a leading `-`, a `/` or a `.`. A path, a
  local `.deb` file or an option is refused.
- You cannot give `apt-get` options. The wrapper owns all of them.

## What to install where

Keep a language dependency out of the system. The Computer already has
`python3` with `uv`, and Node 24 with `pnpm`. Put a package's own dependencies
in `~/software/<name>`: `uv venv` and `uv pip install` for Python, and
`pnpm install` for Node. Use `pagis-apt` only for a system library or for a
command-line program that has no language package.
