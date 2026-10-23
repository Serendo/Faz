# Faz GUI Agent Guide

`faz-gui` owns rendering, window lifecycle, and user interaction.

## Scope

`faz-gui` is responsible for:
- terminal window rendering
- input and mouse handling
- tab/pane UI behavior
- startup lifecycle and app event flow
- the `k` helper binary under `src/bin/k.rs`

## Where to Look

- `src/termwindow/mod.rs`: core window state and action dispatch
- `src/termwindow/render/`: rendering pipeline
- `src/termwindow/mouseevent.rs`: mouse and drag behavior
- `src/termwindow/webgpu.rs`: surface resize and present mode handling
- `src/frontend.rs`: app lifecycle and macOS integration events
- `src/overlay/`: launcher and overlays
- `src/thread_util.rs`: macOS worker thread lifetime helpers
- `src/tabbar.rs`: tab bar behavior

## Practical Rules

- Keep rendering decisions in rendering modules, not scattered across event code.
- Keep initial window dimension math in `TermWindow::new_window()` consistent with `get_os_border()` behavior.
- There is no in-app update surface. The Help menu, the Check/Restart menu items, the background checker, `faz-gui/src/update.rs`, and the `faz update` subcommand were all removed; do not reintroduce them without an explicit product decision. Updates come from downloading a new DMG.
- Keep `FAZ_CONFIG_CHANGED` fast path in `emit_user_var_event()` before pane ownership checks.
- Pass fullscreen state into `TabBarState::new()` and preserve fullscreen-specific title button spacing behavior.
- Wrap macOS worker thread spawns in the existing autorelease-pool helper so shutdown does not reintroduce use-after-free crashes.

## Known Pitfalls

- During window drag, terminal move/wheel events may need suppression.
- WebGPU surface reconfigure should happen only on meaningful state changes.
- Finder "open with" behavior is handled in `frontend.rs` event flow.
- Option+Click cursor movement fires only when: no mouse grab, no alt screen, click is on the same row as the cursor. It sends Left/Right arrow sequences proportional to the column delta; see `mouseevent.rs`.
- Block cursor height uses `natural_cell_height` (ignoring `line_height`) so Nvim visual selections match WezTerm proportions.
- Top border clearance for macOS integrated buttons is state-sensitive:
  - top tab bar visible -> add small gap
  - top tab bar hidden or bottom tab bar -> add larger clearance for traffic lights
- Overlay geometry must update when panes are split, resized, closed, or moved.
- Alternate-screen wheel behavior is config-controlled; do not hard-code one terminal behavior across normal and alternate screens.
- Per-pane overlays render in place of their underlying pane and live in `pane_state`, not the tab's pane tree. They have no window handle, so their output only repaints the window through `PaneOutput` -> `mux_pane_output_event` -> `is_pane_visible` -> `window.invalidate()`. `is_pane_visible` (`termwindow/mod.rs`) MUST treat an active per-pane overlay pane as visible, and the overlay's `render()` MUST `term.flush()`, or streamed output freezes until an input event forces a repaint.
- Scrollback viewport pruning has two policies in `termwindow/mod.rs`: passive pruning during output uses `normalize_viewport` and snaps to the bottom (`None`), while explicit user-driven scroll requests use `normalize_interactive_viewport` and clamp to `scrollback_top`. During a selection drag, only the pane named by `MouseCapture::TerminalPane` may pin a pruned viewport; sibling panes must retain the passive #448 behavior.
- Tab bar rendering and the tab rename editor must read `resolved_palette.tab_bar`, which includes the selected color scheme and user overrides. Reading `config.colors.tab_bar` alone ignores scheme colors and makes Fancy tabs unreadable in light themes.
- Tab bar sizing and hit-testing have pure seams in `src/tabbar.rs`: `tab_width_budget` (per-tab column budget that drives truncation) and `is_tab_hover` (clickable span). The truncation and position regressions (#439/#443/#445) came from layout logic with no test seam, so when you change tab sizing, truncation, or click regions, extend their unit tests instead of only eyeballing the running app. `tab_width_budget` locks the invariant that a tab is never squeezed below 1 column.
- Launcher entries that target a pane must retain its stable `PaneId`, not a tab or topological pane index. The launcher snapshot can outlive pane exits and split-tree reindexing; resolve the `PaneId` against the live mux only after the overlay closes.
- A partially failed startup session restore must preserve the original session envelope and scrollback sidecars. Only mark a snapshot consumed after every saved window restores successfully; otherwise exit-time saving can overwrite the only complete copy.

- ZMODEM pickers are per-pane, not per-tab. `start_overlay_pane` + `assign_overlay_for_pane` render the browser inside its own pane; a tab overlay (`assign_overlay`) cancels whatever that tab already shows, so a broadcast `rz` would leave all but one pane with an abandoned session. `zmodem_pending` is a `HashMap<PaneId, _>` for the same reason: a single slot keeps only the last detection.
- The file browser measures width in display cells, never chars. CJK names are two cells per character; counting chars overflows the column, wraps the line, and shifts everything (cursor included) one row down. Use `unicode_column_width` for truncation, column padding, and caret placement.
- Confirm an overlay-geometry change by what it does at the boundary, not by eyeballing the middle: tab bar hover used to be measured from `border.top` while the bar is painted at `strip - tab_bar_height`, so hovering a real tab could fall outside its region.

## Cross-References

- [`mux/AGENTS.md`](../mux/AGENTS.md) - Tab and pane abstractions consumed by GUI.
- [`termwiz/AGENTS.md`](../termwiz/AGENTS.md) - TUI primitives used in overlays.
- [`config/AGENTS.md`](../config/AGENTS.md) - Config loading and reload signals.
