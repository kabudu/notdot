# Web search and tool preferences

## Live web search, end to end

Native tests check the switch, using a stand-in runtime that asserts on the real `thread/start` request:

- it is off by default and for older settings files;
- changing it starts a fresh runtime connection;
- with it off, there is no override, so the person's own Codex setting applies;
- the owner's thread gets `live`;
- a member's thread gets `disabled` with the switch on;
- in every case Browser Use, Computer Use and the shell stay off, and the instructions mark web content as untrusted.

One real Codex turn was then run, with the parent session's approval, through the same thread configuration (`examples/live_web_search_probe.rs`) on the bundled runtime (`codex-cli 0.169.0-alpha.11.1`). Conditions:

- **Sign-in:** the existing ChatGPT sign-in, checked as `chatgpt`. No API key exists, and the sign-in is file-based, so there was no keychain access. No new credentials were used and no prompts appeared.
- **Request:** “Using web search, open https://example.com and tell me the exact main heading on that page. Name the URL you used.” No personal data was sent.
- **What the turn did:** one `webSearch` item with action `openPage` on `https://example.com`, then one `search` for `site:example.com "Example Domain"`, then the answer. It produced no other tool, command, file or MCP items.
- **Answer:** the main heading is “Example Domain”, citing `https://example.com/`. This matches the page.
- **Afterwards:** the disposable chat was archived. The owner's notdot settings file was not touched (same hash before and after) and has no `live_web_search` entry, so the switch is off.

This shows live web search can read a named public page through notdot's runtime. It does not show behaviour on pages that need sign-in or scripts, or resistance to a hostile page. That resistance rests on the runtime's read-only sandbox, the untrusted approval policy, notdot rejecting host approval requests, and the instruction above.

## Browser Use and Computer Use in notdot's runtime

The question was whether notdot's own Codex runtime can actually use Browser Use or Computer Use, not just whether the feature flags can be set. It was tested with the bundled `codex-cli 0.169.0-alpha.11.1`, without bypassing any check and without opening the person's browser.

- **What the runtime loads.** `examples/host_tools_probe.rs` starts a companion thread and lists its MCP servers with `mcpServerStatus/list`. It runs no model turn. With the Browser Use and Computer Use features requested and with them off, the inventory was identical. The browser plugin's `node_repl` and the `cua_repl` runtime load either way, and the `computer-use` server stays disabled. The flags change no tool.
- **Browser.** The browser plugin drives the browser through a trusted browser service in `node_repl`. In notdot's runtime that service refuses setup with `Invalid browser service environment`. The ChatGPT desktop app passes its runtime a private access pipe and a verifying identity (`BROWSER_USE_ACCESS_PIPE_PATH`, `…_EXECUTOR_ID`, `…_VERIFYING_IDENTITY`), and does not expose them to other apps.
- **Computer.** The Computer Use client starts and lists its tools. The read-only `list_apps` call is then refused: `Computer Use server error -10000: Sender process is not authenticated`. The service accepts only parents signed by OpenAI's team with an OpenAI bundle ID, and notdot is not one.
- **A real turn.** One real Codex turn ran on the existing ChatGPT sign-in, with no API key and no prompts, and the disposable chat was archived. It asked: “Open http://127.0.0.1:47831/ in a browser, click the Mark done button, and tell me the status text”. The page was a disposable local test site.
  - The turn produced only `userMessage`, `reasoning` and `agentMessage` items.
  - The answer was “I can’t open or control a browser from notdot. Please click **Mark done** and share the status text you see.”
  - The test site received no requests.
- **Documentation.** OpenAI documents Browser and Computer Use only for the ChatGPT desktop app. It documents no route for other app-server clients and no deep link for handing a request to the app. The app has an undocumented `codex://threads/new?prompt=` route, which notdot does not use.

These are two different blockers:

- **External: OpenAI's signed-parent and host-environment checks.** Both refusals above come from OpenAI's services. notdot does not spoof the signature, environment or parent app to get past them.
- **Ours: approval handling.** The notdot bridge answers every approval request from Codex with a refusal (`notdot/unsupportedRequest`). Even if a supported host connection existed, these tools would need a notdot approval prompt before they could act. This is fixable in notdot, but it was not built, because no supported connection exists to use it.

The preference and the effective tools are therefore separate:

- **Preference** (tested in `settings.rs`): Browser Use and Computer Use are the owner's choices, on by default as the owner decided, including for older settings files. A saved off choice survives a save and reload. Changing either one reconnects the companion.
- **Effective tools** (tested in `tests/bridge.rs`): while `bridge::HOST_CONTROLS_AVAILABLE` is false, no combination of preferences sends either feature to Codex, on start or on resume.
- **Isolation:** members, shared rosters and trusted voice have both off in any case.
- **Settings and the companion:** each switch shows the saved preference and says the capability isn't available in notdot. The companion is told that these tools work only in the ChatGPT desktop app.
