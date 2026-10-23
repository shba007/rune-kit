# Terminal inventory dashboard

Run `rune-kit tui` explicitly in an interactive terminal. The dashboard never starts automatically, especially during `rune-kit run`, whose stdout remains JSON-RPC only.

The design follows Rune Infer's Ratatui dashboard: cyan accents, muted rounded panels, compact inventory/details and visible keyboard hints. NO_COLOR is supported.

- Up/Down or j/k: choose a plugin; Tab: move focus to/from details.
- Up/Down or j/k in details, or PgUp/PgDn: scroll details. Home resets selection/scroll.
- r: refresh installed metadata and repeat local discovery, after the previous refresh finishes.
- q, Esc or Ctrl-C: leave the dashboard and restore the terminal.

The view stacks its panes on narrower terminals and shows a resize hint below 42 columns × 18 rows. Terminal setup/cleanup uses Ratatui's panic restoration plus an RAII guard for ordinary errors. Worker cancellation is requested after restoring the terminal.

Use `rune-kit tui --once` for plain output or `rune-kit tui --json` for one machine-readable snapshot. Non-interactive invocation without these options returns an actionable error and no terminal output. `--read-only` filters the dashboard's discovered tools; `-p allowed_dir=/absolute/path` supplies a root for its own probes. Defaults from installed metadata and supported host parameters are retained. These options do not change a separate gateway's configuration.

The dashboard distinguishes installed metadata, pending/failed/completed discovery, and a stopped probe. It shows version and resolved executable/storage locations, installed plugin versions and execution type, actual discovery names/descriptions/annotation facts, declared network/executable capabilities, configured roots and missing/unverified managed dependencies. Corrupt metadata and missing payloads are explicit errors rather than empty success. Missing dependencies suggest reinstalling the affected plugin to provision them.

No tools are called, installed or downloaded by this dashboard. Discovery loads installed plugin code locally; native discovery processes are dropped afterwards. Native and WASM discovery calls have a five-second budget and cancellation. Compilation and blocking host callbacks are not covered by that call budget and can delay shutdown, but terminal restoration happens first.

An independent dashboard **does not observe Studio's separate gateway**, active requests, CPU usage, health, history or uptime. A discovered tool is not a promise that a future call will succeed. The configured root displayed is the local probe parameter, not a claim about Studio's permissions. No IPC monitoring mechanism or plugin marketplace is added.

Tests cover empty/corrupt inventory, real native discovery, missing payloads/dependencies, read-only filtering, keyboard selection/scrolling, narrow rendering, non-interactive output, and actual ConPTY/PTY entry/exit after empty inventory and discovery failure. Local terminal evidence is written under target/tui-*-terminal.txt by the PTY test; TestBackend provides a readable frame preview. See the latest handoff report for platforms actually exercised.
