# Changelog
All notable changes to this project will be documented in this file. See [conventional commits](https://www.conventionalcommits.org/) for commit guidelines.

- - -

## [v0.7.0](https://github.com/shba007/rune-kit/compare/v0.6.0..v0.7.0) - 2026-10-24

#### 🚀 Features
- (**generator**) add Cocogitto integration for changelog generation - ([b577fd9](https://github.com/shba007/rune-kit/commit/b577fd9c82ccc8fc7531fa30cd3bf0a399b768d3)) - Shirsendu Bairagi

#### 🐛 Bug Fixes
- (**render**) adjust changelog retrieval to use HEAD for release type - ([4c2458a](https://github.com/shba007/rune-kit/commit/4c2458a546c7560a61ccbeaabe981071f0fb842d)) - Shirsendu Bairagi

- - -

## [v0.6.0](https://github.com/shba007/rune-kit/compare/v0.5.0..v0.6.0) - 2026-10-24

#### 🚀 Features
- ![BREAKING](https://img.shields.io/badge/BREAKING-red) Introduce local MCP HTTP server with attached dashboard - ([e89dc7c](https://github.com/shba007/rune-kit/commit/e89dc7c110a876e4a49dab942519bbbe5e3ecfdd)) - Shirsendu Bairagi

#### 🐛 Bug Fixes
- Change default installation directory to hidden folder - ([69d408f](https://github.com/shba007/rune-kit/commit/69d408fce53bb8494f6a0016e9948506780c9384)) - Shirsendu Bairagi

#### 🧪 Testing
- Enhance Windows hidden install test for path deduplication and identity - ([9186131](https://github.com/shba007/rune-kit/commit/9186131db473a574c3be4952fddf9c6958a4237c)) - Shirsendu Bairagi

#### ⚙️ Miscellaneous
- ![BREAKING](https://img.shields.io/badge/BREAKING-red) introduce local MCP HTTP server with attached dashboard - ([e89dc7c](https://github.com/shba007/rune-kit/commit/e89dc7c110a876e4a49dab942519bbbe5e3ecfdd)) - Shirsendu Bairagi

- - -

## [v0.5.0](https://github.com/shba007/rune-kit/compare/v0.4.0..v0.5.0) - 2026-10-23

#### ⚙️ Miscellaneous
- remove unit tests for release notes extraction, add xtask for release management and changelog handling
- implement MCP 2026-07-28 discovery and per-request metadata model
- Use MCP 2026-07-28 discovery and per-request version/capability metadata on the public stdio gateway. Clients must replace initialization with server/discover and include metadata on every request. Cancelled requests produce no late response; tool schemas, annotation aliases, read-only restrictions and explicit filesystem roots remain supported.
- Install rune-kit under ~/rune and keep managed plugins, skills and provisioned binaries under ~/rune/tools. Copy legacy managed storage without overwriting conflicts, retain originals for recovery, and preserve custom installation paths and Infer files.
- Read native plugin information through the existing private info operation, retaining the legacy probe for older plugins.

- - -

## [v0.4.0](https://github.com/shba007/rune-kit/compare/v0.3.1..v0.4.0) - 2026-10-23

#### ⚙️ Miscellaneous
- Missing WASI filesystem roots no longer preopen the working directory; explicit roots must be existing absolute directories. Legacy annotation output remains enabled.
- Rename the CLI to `rune-kit` and release archives to `rune-kit-<tag>-<target>`. Update MCP client commands from `rune` to `rune-kit`; existing rune-kit data folders are preserved.
- Add verified, per-user Windows and Linux/macOS installers, version pinning and checksum sidecars.
- Self-update accepts both canonical rune-kit and legacy rune archive binaries.

- - -

## [v0.3.1](https://github.com/shba007/rune-kit/compare/v0.3.0..v0.3.1) - 2026-10-21

#### ⚙️ Miscellaneous
- Accept equal canonical and legacy annotation keys together from Rune Tools in native and WASM discovery. Reject conflicting or non-boolean facts with a diagnostic.
- Report discovery failures as JSON-RPC -32000 instead of successful empty/partial lists. Preserve inputSchema and x-rune-rules.
- Retain canonical and legacy output throughout 0.3.x; any output removal requires a minor release and explicit migration note. Legacy input remains supported.

- - -

## [v0.3.0](https://github.com/shba007/rune-kit/compare/v0.2.2..v0.3.0) - 2026-10-21

#### 🚀 Features
- (**protocol**) upgrade MCP annotations to spec 2025-03-26 Hint suffix names (KF-1, KF-3, KF-7) - ([a907af7](https://github.com/shba007/rune-kit/commit/a907af76db749758a60fd886d11e44c3636bf677)) - Shirsendu Bairagi
- Add native MCP fixture and enhance protocol tests - ([8c9a46f](https://github.com/shba007/rune-kit/commit/8c9a46fd64fd5659a1aac38f17aace7875b60529)) - Shirsendu Bairagi
- Enhance runtime error handling and tool cancellation support - ([ce8860a](https://github.com/shba007/rune-kit/commit/ce8860af33c40c97b9f9eed98fd0ab9ea3fc3949)) - Shirsendu Bairagi
- integrate resolve_host_env_params into runtime and core packages - ([98aace8](https://github.com/shba007/rune-kit/commit/98aace83841ae8306e3577d11396a8ff6f8bcf26)) - Shirsendu Bairagi

#### 🚜 Refactoring
- simplify metadata check for executable permissions in package and provision modules - ([17303d3](https://github.com/shba007/rune-kit/commit/17303d37c8a6c2cb218c7793b29ff9a23439e7d0)) - Shirsendu Bairagi

#### 📚 Documentation
- (**handoff**) add Studio follow-up report covering KF-1 through KF-7 - ([53d49d0](https://github.com/shba007/rune-kit/commit/53d49d003562c69a185c086acdb9d833dbd2f752)) - Shirsendu Bairagi
- update README to clarify MCP plugin and skill runtime host description - ([fc44b81](https://github.com/shba007/rune-kit/commit/fc44b81206df0e48895988763c40a7ae9ed8c187)) - Shirsendu Bairagi

- - -

## [v0.2.2](https://github.com/shba007/rune-kit/compare/v0.2.1..v0.2.2) - 2026-09-30

#### 🚀 Features
- enhance skill detection and improve executable resolution logic - ([dac0c5b](https://github.com/shba007/rune-kit/commit/dac0c5bdfdf0943e5ff4a2c2eac182653efca8f7)) - Shirsendu Bairagi

#### ⚙️ Miscellaneous
- bump version to 0.2.2 for rune-kit-cli and rune-kit-core - ([bee83f0](https://github.com/shba007/rune-kit/commit/bee83f0b0e877257de98e14bcc73031fb3f5bd46)) - Shirsendu Bairagi
- (**version**) 0.1.0 - ([5d3025b](https://github.com/shba007/rune-kit/commit/5d3025bad903ee7f1fc05b87c31e97256420b589)) - github-actions[bot]

- - -

## [v0.2.1](https://github.com/shba007/rune-kit/compare/v0.2.0..v0.2.1) - 2026-09-29

#### 🐛 Bug Fixes
- rename manifest field to _manifest in NativeSidecar struct - ([398b765](https://github.com/shba007/rune-kit/commit/398b76546212d76060f330e9192b7e4d57a80250)) - Shirsendu Bairagi

#### ⚙️ Miscellaneous
- remove unused resolver setting from Cargo.toml and update lefthook.yml - ([cb0b704](https://github.com/shba007/rune-kit/commit/cb0b704d0002b79162b099132fd2fb3c8949600b)) - Shirsendu Bairagi
- (**version**) 0.1.0 - ([9403712](https://github.com/shba007/rune-kit/commit/94037123db4b379d4df4eb98800ecb3b8cf76506)) - github-actions[bot]
- bump version to 0.2.1 and update thiserror dependency to 2.0.21 - ([44fc569](https://github.com/shba007/rune-kit/commit/44fc569349bc478d87447ffb2c0ac44e3fd70f5b)) - Shirsendu Bairagi
- (**version**) 0.1.0 - ([28dc378](https://github.com/shba007/rune-kit/commit/28dc378280bfe14a334960ed4207ac2a1f85a8f8)) - github-actions[bot]

- - -

## [v0.2.0](https://github.com/shba007/rune-kit/compare/v0.1.5..v0.2.0) - 2026-09-21

#### 🚀 Features
- enhance package management with binary provisioning and manifest support - ([4346e4a](https://github.com/shba007/rune-kit/commit/4346e4a834340e6e7bd04aea370a3a60653f9b90)) - Shirsendu Bairagi
- update CI workflow and add pre-commit hooks for code quality checks - ([0dd707a](https://github.com/shba007/rune-kit/commit/0dd707a1b61ec60e2ffc6ef6f96d25dfb24eb21f)) - Shirsendu Bairagi
- enhance target triple detection for cross-platform compatibility - ([386611e](https://github.com/shba007/rune-kit/commit/386611efbe9f2ca2dc95e3dcb5b6bcaa028e6618)) - Shirsendu Bairagi

#### 🚜 Refactoring
- streamline command handling and improve error reporting in CLI - ([e611785](https://github.com/shba007/rune-kit/commit/e6117857308281ae2b79b2e57f5f23b4601069ff)) - Shirsendu Bairagi
- simplify environment variable handling and improve parameter management in runtime - ([8f54e5e](https://github.com/shba007/rune-kit/commit/8f54e5eeb2519d992e0ccd8984d289e86fd76d55)) - Shirsendu Bairagi

#### ⚙️ Miscellaneous
- apply code fixes [skip ci] - ([340be1c](https://github.com/shba007/rune-kit/commit/340be1c1c1f494b025734071374e5f102243cb99)) - shba007
- (**version**) 0.1.0 - ([c8f2b38](https://github.com/shba007/rune-kit/commit/c8f2b3812a004235c7dcd94cc392a9a195642729)) - github-actions[bot]
- (**version**) bump package version to 0.2.0 - ([8d65220](https://github.com/shba007/rune-kit/commit/8d65220c4edb4eb53b5d64f1c77284fe05047620)) - Shirsendu Bairagi
- (**version**) 0.1.0 - ([d38f6a0](https://github.com/shba007/rune-kit/commit/d38f6a0f905b38fd11edbd744e4f8bbc3d4a4cb3)) - github-actions[bot]

- - -

## [v0.1.5](https://github.com/shba007/rune-kit/compare/v0.1.4..v0.1.5) - 2026-09-17

#### 🚀 Features
- add detailed documentation for agents, architecture, and usage in README and AGENTS.md - ([3b38966](https://github.com/shba007/rune-kit/commit/3b38966b7b931111ec0f630aa8095784ec5341b6)) - Shirsendu Bairagi
- implement Column and Table structures for output formatting - ([9d9a96a](https://github.com/shba007/rune-kit/commit/9d9a96ad960f3bec3592a7a6d8c5448ec45cc8e6)) - Shirsendu Bairagi
- restructure CLI by separating main logic into library and enhancing output rendering functions - ([bd42ebf](https://github.com/shba007/rune-kit/commit/bd42ebfc30031dd97da3dc5934e2a452c41f4849)) - Shirsendu Bairagi
- enhance CLI and core functionality with resource and prompt management, including URI parsing and new data structures - ([57b94d3](https://github.com/shba007/rune-kit/commit/57b94d398c429c5243e9b00747b8866f73acbeef)) - Shirsendu Bairagi
- implement skill management with installation, uninstallation, and update functionalities, including registry integration and unified output rendering - ([91538f7](https://github.com/shba007/rune-kit/commit/91538f7075acde66ad0dceddfef54097901b9433)) - Shirsendu Bairagi
- add dual binary installation support for plugins, including detection and installation of both WASM and native binaries - ([0320464](https://github.com/shba007/rune-kit/commit/032046450101e36e2e86ccd579942ed7c8793318)) - Shirsendu Bairagi
- implement protocol handling and runtime plugin management - ([ae056cd](https://github.com/shba007/rune-kit/commit/ae056cd6e3e9a0808a97ce83e7b3d463305c78e0)) - Shirsendu Bairagi

#### 🚜 Refactoring
- clean up project structure by removing unused mcp.json configuration file - ([422c65c](https://github.com/shba007/rune-kit/commit/422c65c4c0cb3fd4b91a80b08b69381f14e3bd12)) - Shirsendu Bairagi
- wire registry/update/List render paths onto Table/Column - ([5f1409b](https://github.com/shba007/rune-kit/commit/5f1409bc25dd02ab9786b4004e7283049f19a152)) - Shirsendu Bairagi
- improve code readability by adjusting conditional formatting and removing unnecessary line breaks - ([8191dd3](https://github.com/shba007/rune-kit/commit/8191dd34116acc59bb4568685f56f4066991d896)) - Shirsendu Bairagi

#### 🤖 Continuous Integration
- (**release**) update version - ([d46d471](https://github.com/shba007/rune-kit/commit/d46d4713bcee91f8e7f16515375f2d3842340287)) - shba007

- - -

## [v0.1.4](https://github.com/shba007/rune-kit/compare/v0.1.3..v0.1.4) - 2026-09-03

#### 🚀 Features
- enhance JSON-RPC response structure with error handling and content formatting - ([fdf192b](https://github.com/shba007/rune-kit/commit/fdf192b7dec28b96ab26388749b2602945b34847)) - Shirsendu Bairagi
- add commands for listing available tools and searching registry; enhance update functionality - ([4314369](https://github.com/shba007/rune-kit/commit/431436947661bb5317f0440c9d27799cd108e22b)) - Shirsendu Bairagi

#### 🐛 Bug Fixes
- update deploy and integrate workflows for improved artifact handling and linting - ([2bd8a9f](https://github.com/shba007/rune-kit/commit/2bd8a9fd644cfe26c29def9769abe05821412d6b)) - Shirsendu Bairagi
- update installation instructions in README for clarity - ([8005228](https://github.com/shba007/rune-kit/commit/8005228e99a13e5f034b9114f047e7b07e91950b)) - Shirsendu Bairagi

#### 🤖 Continuous Integration
- (**release**) update version - ([1327484](https://github.com/shba007/rune-kit/commit/1327484b456fbc2a63df8f7fffeafe7a935ddc1e)) - shba007

- - -

## [v0.1.3](https://github.com/shba007/rune-kit/compare/v0.1.2..v0.1.3) - 2026-09-02

#### 🚀 Features
- enhance plugin management with native execution support - ([c0ab4c3](https://github.com/shba007/rune-kit/commit/c0ab4c33ac2ab63f8fb4ca2838d35a9780e2ed00)) - Shirsendu Bairagi

#### 🤖 Continuous Integration
- (**release**) update version - ([891af35](https://github.com/shba007/rune-kit/commit/891af351f848515eef3747e4d451dfbe0bd10135)) - shba007

- - -

## v0.1.2 - 2026-09-01

#### 🚀 Features
- initialize rune-kit workspace with core and CLI crates - ([993ad33](https://github.com/shba007/rune-kit/commit/993ad3381531eadf0ebb7284e5ba3964956ddc31)) - Shirsendu Bairagi
- add README, and enhance protocol handling - ([5342a6a](https://github.com/shba007/rune-kit/commit/5342a6af54fe49e04e2002824f5f973666635616)) - Shirsendu Bairagi
- enhance plugin management with metadata support and update README - ([42c2129](https://github.com/shba007/rune-kit/commit/42c2129422b2e3709edaa122b0811edb227ef452)) - Shirsendu Bairagi
- add test prompts and CLI configuration to README and introduce mcp.json for server commands - ([44814c6](https://github.com/shba007/rune-kit/commit/44814c699dad0dd05bc4ff18cf4aa096d53fbe20)) - Shirsendu Bairagi
- add git execution functionality and update mcp.json for rune-git command - ([68b4c9e](https://github.com/shba007/rune-kit/commit/68b4c9ecabadce589ac15847b2dc1e4ec1af0a55)) - Shirsendu Bairagi
- add rune-memory functionality with entity creation and graph reading - ([e30fc6f](https://github.com/shba007/rune-kit/commit/e30fc6f4be9ef981501e4f574fc28301f4069aa2)) - Shirsendu Bairagi
- add sequential thinking functionality and update README and mcp.json - ([1ae505a](https://github.com/shba007/rune-kit/commit/1ae505a19f467a1a595b5b1d01edfcc0dcde4483)) - Shirsendu Bairagi
- add video metadata inspection tool and update mcp.json for new commands - ([143f9d7](https://github.com/shba007/rune-kit/commit/143f9d77897adcd62244a65b433c40d95b4c3054)) - Shirsendu Bairagi
- (**cli**) support host env vars for plugin config and bump to 0.1.1 - ([61c818d](https://github.com/shba007/rune-kit/commit/61c818d5f9ecd5dd5b0e529bf7415734758dcfe6)) - Shirsendu Bairagi

#### 🚜 Refactoring
- (**cli**) inject all host env vars as lowercase plugin params - ([e918816](https://github.com/shba007/rune-kit/commit/e918816b7dc85a1f9b08779d801186b50ba39c3d)) - Shirsendu Bairagi

#### 🤖 Continuous Integration
- add release pipeline and harden plugin installation - ([396203c](https://github.com/shba007/rune-kit/commit/396203c9c4ebe47fc1495ec9fa565e4d221ae5e7)) - Shirsendu Bairagi
- (**release**) update version - ([eb4b02f](https://github.com/shba007/rune-kit/commit/eb4b02f067090573193d2737a47e89937de3aee1)) - shba007

- - -

Changelog generated by [cocogitto](https://github.com/cocogitto/cocogitto).
