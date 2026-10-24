# Kit terminal dashboards

`rune-kit serve` starts the local HTTP server and opens its attached Ratatui
TUI when stdin/stdout are interactive. `--headless` disables the TUI explicitly;
non-interactive serving works automatically without terminal control sequences.
Server availability never depends on an attached terminal.

The attached dashboard reads this process's shared observed state: actual bound
endpoint, listening/stopping status, uptime, loaded plugin versions/runtime types,
discovery readiness, queued/running calls, recent outcomes and durations. History
is bounded to 64 completed operations. It shows read-only, server allowlist,
filesystem roots and deadline settings. Discovery states update when a client
actually lists tools; loading alone is not reported as successful discovery.
Only operation names/status are displayed, not arguments, result content,
authorization tokens or arbitrary runtime errors. It does not monitor Studio or
another Kit process and invents no CPU/memory/health figures.

Controls: arrows/j/k select recent calls, r refresh, q/Esc/Ctrl+C stop the server.
Frames refresh from actual state every 100 ms; refresh does not install plugins
or issue independent tool calls. Resize redraws automatically. The terminal
restoration guard is installed around normal exit and render/input error paths;
normal exit/discovery-error restoration was executed. Forced input/render errors
and panic cleanup are NOT CERTAIN (not separately exercised). Server shutdown
cancels its operations. Native descendants/blocking host
callbacks may delay complete process exit, without retaining the terminal UI.

Standalone `rune-kit tui` remains an inventory/details/diagnostics view, using
arrows/j/k, Tab, PgUp/PgDn, r and q. It loads and probes installed plugins for
its own inventory; it does NOT attach to an independently running server.
`rune-kit tui --once` and `--json` are non-interactive snapshots. Use the attached
serve dashboard to see server operations instead.

PTY tests cover standalone empty/failed discovery and attached server entry/exit
with terminal restoration. TestBackend covers empty/corrupt/narrow layouts.
See docs/handoff/http-server-report.md for actual platform/test evidence.
