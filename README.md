# nvim-ui (Rust port)

A greenfield, cross-platform, fully native **Rust** Neovim GUI client. It embeds
`nvim --embed`, renders the editor grid on the GPU via `wgpu`, and drives a
frame-delta animation engine (cursor motion, trail, glow, squash/stretch, smooth
scroll, flashes, float fade, optional power mode).

This is the Rust implementation of the design in [`../AGENT_RUST_PORT.md`](../AGENT_RUST_PORT.md).

## Workspace layout

```
crates/
  nvim-core      # RPC session (nvim-rs), UI protocol decode, grid/window state, input encoding
  editor-surface # wgpu renderer, glyph atlas, frame builder, animation engine, WGSL shaders
  app            # winit shell: window lifecycle, settings, input/mouse/IME/resize wiring
```

The split keeps `nvim-core` free of any windowing/GPU dependency so its protocol,
grid, and input logic are unit-testable in isolation.

## Build & run

Requires a recent Rust toolchain and `nvim` on `PATH`.

```bash
cargo run -p app           # or: cargo run --bin nvim-ui
RUST_LOG=info cargo run -p app
cargo test -p nvim-core    # framing/protocol/input unit tests
```

Settings persist to the OS config dir (`settings.json`): font family/size,
line-height, animation toggles, power mode.

## Stack

| Concern | Crate |
|---|---|
| Windowing / event loop / IME | `winit` |
| GPU | `wgpu` (Metal / Vulkan / DX12) |
| Async + nvim I/O | `tokio` |
| Neovim embedding + msgpack-RPC | `nvim-rs` (streaming decoder — never re-encodes to advance the buffer) |
| Font rasterization / discovery | `fontdue` + `fontdb` |
| Atlas packing | `etagere` |
| Settings | `directories` + `serde_json` |

## Architecture

```
winit event loop (app)
  └─ NvimSession (spawn `nvim --embed`, msgpack-RPC over stdio, tokio task)
        └─ redraw notifications forwarded to the UI thread via EventLoopProxy
              └─ parse_redraw -> Vec<UiEvent>  (unknown events/args ignored)
                    └─ GridStateStore.apply_batch  (present only on `flush`)
                          └─ FrameBuilder -> rect/glyph instance draw lists
                                └─ Renderer (wgpu): rects → glyphs, one pass
  └─ AnimationState: logical vs animated cursor, idle gate (0% CPU when still)
```

## v1 status

Implemented: single-grid (`ext_linegrid`) rendering with full hl attributes,
double-width cells, box-drawing as rects, decorations; block/vertical/horizontal
cursor with blink; the full animation engine behind runtime-toggleable config
with the idle gate; keyboard (Neovim notation), mouse + wheel + drag, IME commit,
resize, focus, HiDPI (atlas reset on DPI/font change); crash-safe child teardown.

Designed-for-but-deferred (per the spec): `ext_multigrid`, external
cmdline/popupmenu/messages widgets, command palette / settings chrome, ligatures,
remote socket attach. The grid state is keyed by grid id so multigrid can be
enabled without restructuring.
