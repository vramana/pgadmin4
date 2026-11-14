# pgAdmin 4 native shell (Rune)

A Rust replacement for the Electron runtime in `../runtime`, built with
[Rune](https://github.com/anomalyco/rune). This is phase 2 of the migration plan:
the shell and the Flask sidecar. The native workspace UI is ported onto it next.

What it does today:

- picks a free port on 127.0.0.1 and a one-time key;
- starts `python -s web/pgAdmin4.py` with `PGADMIN_INT_PORT`, `PGADMIN_INT_KEY` and
  `PGADMIN_SERVER_MODE=OFF`, as `runtime/src/js/pgadmin.js` does;
- pings `/misc/ping?key=…` (100 ms doubling to 1 s, 180 s timeout) until the server answers;
- shows the server's state and log in a Rune window, with "Open in browser" once it is up
  and "Retry" if it fails;
- stops the server when the window closes.

## Run

Rune needs Rust 1.99.1 (pinned in `rust-toolchain.toml`). On Linux install
`libxkbcommon-dev libwayland-dev` first.

```sh
cd native
PGADMIN_PYTHON=/path/to/venv/bin/python cargo run
```

`PGADMIN_PYTHON` defaults to `python3` on PATH, and `PGADMIN_WEB_FILE` to `../web/pgAdmin4.py`.

## Test

```sh
cargo test                                                        # unit tests
PGADMIN_PYTHON=/path/to/venv/bin/python cargo test -- --ignored   # starts the real server
./target/debug/pgadmin4 --snap ready.png --scene ready            # headless render
```

Headless renders without a GPU need a software Vulkan driver
(`mesa-vulkan-drivers`, then `VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json`).

## Not done yet

- Bundled Python and `web/` paths for packaged builds (`runtime/src/js/misc.js` layouts),
  the Windows `PATH` entry for libpq, and the fixed-port and timeout settings from the
  Electron config store.
- A server log file on disk.
- Native menus, file dialogs and downloads (the `window.electronUI` bridge).
- Packaging in `pkg/` and auto-update.
