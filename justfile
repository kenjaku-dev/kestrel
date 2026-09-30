# Kestrel's build, test and packaging tasks.
#
# Run `just` with no arguments for the list. Everything that touches the user's
# home does so under `PREFIX`, which defaults to the XDG data dir, so
# `just install` needs no root and touches nothing outside it.
#
#   just            list the targets
#   just run        build and run the file manager
#   just test       fmt, clippy, and the whole test suite
#   just release    the stripped binary, with its size
#   just install    binary, .desktop and icon into ~/.local/share
#
# The three install locations are each overridable — `PREFIX`, `DESKTOP_DIR`,
# `ICON_DIR`, and `STATE_DIR` for the preferences — so the whole thing can be
# pointed at a temporary directory and tried without touching the real one.
# `PREFIX=/tmp/x DESKTOP_DIR=/tmp/y ICON_DIR=/tmp/z just install` writes
# nothing outside /tmp.
#   just uninstall  the same three files, and nothing else
#   just screenshots  render every scene in both themes
#
# The explanation for each target is in the comment block above it; the one
# immediately above a recipe is its short description, which is why some of
# these blocks are split in two.

set shell := ["bash", "-uc"]

# The freedesktop data directory.
#
# `XDG_DATA_HOME` is honoured when it is set. The spec also says a *relative*
# one must be ignored, and `env_var_or_default` cannot test that, so the
# freedesktop rule is documented rather than enforced: a relative
# `XDG_DATA_HOME` here resolves against the caller's working directory, which
# is the one input that would make `just install` write somewhere surprising.
xdg_data_home := env_var_or_default("XDG_DATA_HOME", "")
data_home := if xdg_data_home != "" {
    xdg_data_home
} else {
    home_dir() / ".local" / "share"
}

# `dev.kestrel.app` is the `app_id` in `main.rs` **and** the base of the state
# directory, because eframe derives `~/.local/share/<app_id>/app.ron` from that
# string. Two names for one thing is a bug waiting to happen, so it is one
# variable.
app_id := "dev.kestrel.app"

# The install prefix for the *binary*, and the only way this file can write
# outside the XDG data directory. Every path in every target is built from it,
# so `PREFIX=/tmp/x just install` is a complete sandbox and `PREFIX=/ just
# uninstall` cannot be pointed at a real system by accident.
#
# `env_var_or_default` reads the **environment**, so the override is
# `PREFIX=... just install` — not a command-line argument, which `just` refuses
# for a variable it does not define as a recipe parameter.
#
# Deliberately *not* the same directory as `state_dir` below. They were, and
# `uninstall` then carried an `rmdir` of the directory holding the user's
# settings; harmless only because `rmdir` refuses a non-empty directory, which
# is exactly the kind of safety that stops being harmless the first time a
# setting file moves.
prefix := env_var_or_default("PREFIX", data_home / "kestrel")

bin_dir := prefix / "bin"

# The two XDG paths, each independently overridable, because a *complete*
# sandbox has to move all three: with only `PREFIX` overridable, `just install
# PREFIX=/tmp/x` still wrote into the real `~/.local/share/applications`, which
# makes the sandbox a lie and makes "just try it" mean "just install it".
icon_dir := env_var_or_default("ICON_DIR", data_home / "icons" / "hicolor" / "scalable" / "apps")
desktop_dir := env_var_or_default("DESKTOP_DIR", data_home / "applications")
state_dir := env_var_or_default("STATE_DIR", data_home / app_id)

# Where `just screenshots` writes. A screenshot is a build artifact, not a
# source file, so it does not belong in the tree.
OUT := "/tmp/kestrel-shots"

# Print the target list.
default:
    @just --list --unsorted

# Build and run the file manager.
#
# `cargo run` on a workspace member needs `-p`, because `-p kestrel` and the
# binary of the same name are different selectors and cargo will not guess.
run *ARGS:
    cargo run -p kestrel -- {{ARGS}}

# Everything the gates are: format, lint, test.
test:
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets -- -D warnings
    cargo test --workspace

# The stripped release binary, and its size.
#
# The size is the point. "Lightweight" is a claim, and the number is the only
# part of it a user can check. `strip = "symbols"` is in the release profile, so
# the file cargo produced is the file that ships — there is no separate `strip`
# step that could be forgotten, and no way for the reported figure and the
# shipped figure to differ.
release:
    cargo build --release
    @printf 'binary: %s\n' "$(target/release/kestrel)"
    @ls -l target/release/kestrel | awk '{printf "size:   %.1f MB (stripped)\n", $$5/1048576}'

# Install into `prefix`. No root, nothing outside it and the three XDG paths.
install:
    cargo build --release
    install -Dm755 target/release/kestrel "{{bin_dir / "kestrel"}}"
    # The desktop entry is a **template**: its `@@BIN@@` placeholder is replaced
    # with the absolute path of the binary that was just installed. A bare
    # `Exec=kestrel` looks right and is not — a launcher's PATH is whatever the
    # session it was started from had, which is not this shell's PATH and not
    # next login's.
    #
    # Written to a temporary file and moved into place, so a desktop environment
    # watching `applications/` never reads a half-written entry. And the
    # substitution is checked: a `@@BIN@@` that survives into the installed file
    # would produce an entry that silently does nothing, and the only place to
    # find that out is a launcher three weeks later.
    @install -d "{{desktop_dir}}"
    @install -d "{{icon_dir}}"
    @sed 's|@@BIN@@|{{bin_dir / "kestrel"}}|g' kestrel/packaging/kestrel.desktop \
        > "{{desktop_dir / ".kestrel.desktop.new"}}"
    @if grep -q '@@BIN@@' "{{desktop_dir / ".kestrel.desktop.new"}}"; then \
        echo "install: @@BIN@@ survived the substitution; aborting" >&2; \
        rm -f "{{desktop_dir / ".kestrel.desktop.new"}}"; exit 1; \
    fi
    install -m644 "{{desktop_dir / ".kestrel.desktop.new"}}" "{{desktop_dir / "kestrel.desktop"}}"
    @rm -f "{{desktop_dir / ".kestrel.desktop.new"}}"
    install -Dm644 kestrel/packaging/icons/hicolor/scalable/apps/kestrel.svg "{{icon_dir / "kestrel.svg"}}"
    # eframe's state directory, created now rather than on first exit, so a
    # permissions problem shows up at install time and not at quit time.
    install -d -m 700 "{{state_dir}}"
    @printf 'installed:\n'
    @printf '  %s\n' "{{bin_dir / "kestrel"}}"
    @printf '  %s\n' "{{desktop_dir / "kestrel.desktop"}}"
    @printf '  %s\n' "{{icon_dir / "kestrel.svg"}}"
    @printf '\nsettings will be written to %s\n' "{{state_dir / "app.ron"}}"
    @printf '\nAdd to PATH with:\n  export PATH="%s:$PATH"\n' "{{bin_dir}}"

# Remove exactly what `install` put there.
#
# `rm -f` on three named paths, never a glob: `uninstall` has to be able to
# undo `install` exactly, and a glob would take anything else that happened to
# match. The state directory is deliberately **left alone** — it holds the
# user's settings, and an uninstall that silently deletes them is a program
# that destroys user data as a side effect. It is listed, and left.
uninstall:
    @printf 'about to remove:\n'
    @printf '  %s\n' "{{bin_dir / "kestrel"}}"
    @printf '  %s\n' "{{desktop_dir / "kestrel.desktop"}}"
    @printf '  %s\n' "{{icon_dir / "kestrel.svg"}}"
    @rm -f "{{bin_dir / "kestrel"}}"
    @rm -f "{{desktop_dir / "kestrel.desktop"}}"
    @rm -f "{{icon_dir / "kestrel.svg"}}"
    @rmdir "{{bin_dir}}" 2>/dev/null || true
    # `rmdir` on the prefix only, and never on `state_dir`: `rmdir` refuses a
    # non-empty directory, so a settings file is safe, and the settings
    # directory is not touched at all.
    @rmdir "{{prefix}}" 2>/dev/null || true
    @if [ -f "{{state_dir / "app.ron"}}" ]; then printf '\nleft alone: %s (your settings; delete it yourself if you mean to)\n' "{{state_dir / "app.ron"}}"; fi

# Render every scene in both themes, and report the file sizes.
#
# This is the only way to check this app's appearance that does not involve
# whatever else is on the desktop. It is the same code path `just scenes` runs,
# only this one writes the PNGs out where a human can look at them.
screenshots:
    mkdir -p {{OUT}}
    -bash -c 'set -e; for scene in browser confirm-permanent confirm-trash collision progress failed preview-empty preview-text preview-image preview-too-large settings help empty denied gone nowatch; do for theme in light dark; do target/debug/kestrel --screenshot "{{OUT}}/$scene-$theme.png" --scene "$scene" --$theme >/dev/null; done; done'
    @printf 'wrote %s screenshots to %s\n' "$(ls {{OUT}} | wc -l)" "{{OUT}}"
    @ls -l {{OUT}} | awk 'NR>1 {printf "  %-36s %5.0f KiB\n", $$9, $$5/1024}'

# Every scene rendered, as a test rather than a screenshot.
#
# Faster than `just screenshots` and it is the one that runs in CI, so a scene
# that cannot be driven into its state fails the build rather than producing a
# PNG of the wrong thing.
scenes:
    cargo test -p kestrel --bin kestrel every_scene_renders

# Phase 1 scaffold: run the Tauri shell in dev mode (Vite on :1420).
run-tauri:
    cargo tauri dev

# Install the Tauri CLI locally (pinned to the scaffold's Tauri version).
install-tauri:
    cargo install tauri-cli --version "^2" --locked
