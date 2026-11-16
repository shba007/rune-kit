# Rune Kit — Goal

> This file states direction, not status. What works today is in `README.md`, `AGENTS.md` and `TODO.md`, and the
> code is the source of truth over any document. Decisions recorded here were
> made by the owner on 2026-11-16.
## The whole goal

Rune is a local, Jarvis-like AI harness. You talk or type, or it proposes work
itself; it does the work on your own devices with your own models, tools and
memory; it shows the real result of every step while it works; and it asks
before anything risky.

### Principles

1. **Local first, with a fixed fallback order.** A model runs on the device in
   use when it fits. If it does not fit, it runs on the user's own PC. If it
   does not fit there, it runs in the cloud. If all three fail for that model,
   the next equivalent model is tried in the same order. A cloud-only model
   goes straight to the cloud. Cloud is optional in general and is the expected
   path only for devices too weak to run models (laptop, phone, later IoT).
2. **Strict ownership.** Each concern has exactly one owner (table below).
3. **Three kinds of step, one owner each.** A model call belongs to Infer, a
   tool call to Kit, a screen action to Studio. A tool or model call is
   preferred; a UI Automation element comes next; pixels are last.
4. **Show the thing, not a description.** Every step shows what it actually
   produced (the email text, the table rows, the photos) and decisions are made
   on that content. The view is generated automatically from the schema the
   owner publishes: Kit for tools, Infer for model operations. No model
   generates a layout, and screen captures are not used as a substitute view.
5. **Live and replayable.** Steps appear as they start. The same event stream
   drives the live view and playback. The pill shows a glance; a workbench
   window shows and decides.
6. **Decisions are choices.** Several options, one suggested, each showing its
   outcome on the real content. Not a bare yes or no.
7. **A model per step.** Studio chooses which model a step needs, by the step's
   difficulty and the speed required. Infer decides where that model runs,
   following principle 1.
8. **Sub-agents when the goal needs them.** The plan shown to the user states
   whether one agent or an orchestrator with several sub-agents will do the
   work. Simple requests use one agent. When sub-agents are used they work in
   parallel wherever their work does not conflict.
9. **Autonomous by proposal.** Rune looks for useful work without being asked,
   by observing through tools (inbox, calendar, files), never by watching the
   screen continuously. By default it only suggests. Preparing work for
   approval is opt-in per area. Content from outside never raises what it may
   do unasked.
10. **Ask before assuming.** When a request lacks something needed to plan, or
    contradicts itself, Rune asks the user directly before planning: few
    questions, each with options and a suggested answer. It does not ask what
    it can look up or safely default, and it does not plan on a guess.
11. **Plans can grow.** An approved plan is a starting point. While working, a
    step may be added when a need is discovered, such as looking something up,
    fetching a file or producing something with a model. Added steps are shown
    as added and pass the same safety gate as planned ones.
12. **Shared materials with provenance.** Everything gathered or produced for a
    goal is kept in one place that every step and sub-agent can reuse, so the
    same item is not fetched or produced twice. Each item records where it came
    from: its source and licence if fetched, the model if generated. Claims
    taken from outside carry their source. Requests that leave the machine,
    such as a web search, must not contain sensitive data.
13. **Drafts, not restarts.** A result is kept as parts that depend on each
    other. The user can give feedback on any part; only what depends on that
    part is redone, and the outcome is a new draft. Earlier drafts are kept for
    comparison and return.
14. **Reuse before building.** Existing engines, MCP servers, research and
    projects are preferred over new code.
15. **Small cycles.** Changes ship as small, deployable steps ordered by the
    80/20 rule.
16. **Safe by default.** Risky actions need confirmation, everything is
    audited, and sensitive data classes stay on the user's own hardware.
17. **Scope for now.** Windows, Linux and Android. Both a personal tool and a
    public product.
18. **A non-goal is not a removal order.** Code that falls outside these goals
    stays as it is. Nothing is removed, disabled or rewritten because it is a
    non-goal unless the owner explicitly approves that removal.

### Ownership

| Repo | Owns |
| --- | --- |
| **rune-studio** | The agent runtime and orchestrator, the user interface, the safety gate, screen actions, voice, the run trace, and the bridge that lets outside agents drive the screen |
| **rune-infer** | Every model and engine, including specialist operations (OCR, detection, generation, speech), and where a model runs: device, home PC or cloud |
| **rune-kit** | Every MCP server the harness uses (first-party plugins, third-party stdio and HTTP servers), skills, the sandbox, and shared external binaries such as ffmpeg |
| **rune-tools** | First-party plugins for Kit |
| **rune-lake** | Memory: the media catalog, long-term memory and the encrypted life store |
| **rune-agent** | Sub-agents (personalities) and their workflows: a prompt, skills, MCP servers and a workflow each |
| **rune-model** | The model weight store, nothing more |
## This repo's goal

Kit is the only thing in Rune that runs the MCP servers the harness uses, in
the same way Infer is the only thing that runs a model. Every tool, whoever
wrote it, is started, sandboxed, listed and stopped by Kit and reached through
Kit's one endpoint. Kit is also the source of each tool's schema, from which
Studio generates the view of a result.

### Current goals

1. **Host third-party stdio MCP servers.** Install one from a command and
   arguments, run it as a supervised child, and expose its tools through the
   same endpoint and namespace rules as Kit's own plugins.
2. **Isolation that is actually on.** Enable native process isolation on
   Windows and Linux so a third-party process does not run with the user's full
   rights.
3. **Schemas as the view contract.** Serve each tool's input and output schema,
   validate every result against it, and carry optional author-written display
   hints. For a third-party server with no output schema, allow a hand-written
   schema overlay; a model never writes one.
4. **Preview and undo.** Extend staged changes so a tool can describe a change
   before making it, and report calls, arguments and staged changes to Studio.
5. **Safety markers.** Read-only and destructive markers on every tool, which
   Studio uses for confirmation and for what may run unasked.
6. **Events from tools.** A way for a server to signal that something changed
   (new mail, new file), so Rune can propose work without polling.
7. **Serve sub-agents.** Install the skills and MCP servers a sub-agent names.
8. **One provider of shared binaries.** Infer and Lake obtain ffmpeg from Kit.
9. **Provenance on results.** A tool that brings something in from outside
   reports its source, and its licence where known, in a standard place in the
   result.

### Not goals

- Running models, or wrapping Infer's model operations as tools.
- Hosting Studio's outside-agent bridge; that stays in Studio.
- Holding memory (Lake) or deciding what an agent does (Studio).
- Generating or choosing layouts.
- New work on the standalone terminal dashboard.

Per principle 18, existing code in these areas is not to be removed without the
owner's explicit permission.

### Where the code is today

- One Streamable HTTP MCP endpoint multiplexing tools, resources and prompts.
- Two plugin kinds in Kit's own format: WebAssembly in an Extism sandbox, and
  native executables over private stdio. Third-party servers only as remote
  Streamable HTTP endpoints.
- A package manager for plugins and skills, a registry, staged file changes
  with backups, and a management API that Studio uses.
- Native isolation exists per platform but is switched off pending validation.
- No push channel from servers, and ffmpeg is provisioned for plugins only.

### Open questions

- How a third-party stdio server is assigned filesystem and network limits when
  it ships no manifest.
- Whether to accept more than one MCP protocol revision.
- How Kit runs on Android, or whether Android only uses a Kit on the home PC.
