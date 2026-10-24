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

The standalone tui command has been removed. Use serve for both inventory and
server operations. Tab switches calls/tools; d discovers through this server's
HTTP endpoint using the same permissions and plugin instances. Discovery is bounded
and does not run tools. r redraws observed state. Headless serving is unchanged.

The attached server dashboard displays the actual running executable and resolved
managed data root. Defaults are `~/.rune/rune-kit[.exe]` and `~/.rune/tools`;
custom executable locations and `RUNE_KIT_DATA_DIR` are shown as resolved, rather
than inferred from defaults. It observes only its own HTTP server.

The dashboard follows Rune Infer's terminal theme: dim rounded borders, cyan
headings and focus, white normal status, red failures and compact key chips.
The main view separates plugin inventory from live/recent requests. Press i for
configuration, restrictions and full executable/data paths; Tab returns to the
calls/tools view. At narrow widths the server summary remains available in configuration;
terminals below 48 columns or 16 rows show a resize hint. NO_COLOR and RUNE_ASCII
apply to the complete frame, as in Infer. No CPU/health metrics are invented.
