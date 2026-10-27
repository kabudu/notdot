# notdot

This is a technical record of the Desktop connection tests. The practical result: notdot sends messages through its own Codex process; Desktop can show saved history. Two apps cannot safely send to the same chat at once. For everyday use, see [Settings](settings.md).

Private feasibility prototype for a desktop voice companion. No voice, orb, memory ledger or onboarding has been built yet.

## Bridge proof — 26 October 2026

Tested against the Codex runtime bundled with ChatGPT Desktop, `codex-cli 0.164.0-alpha.11.1`, using the existing local ChatGPT sign-in. No API key was copied or required.

| Requirement | Result |
| --- | --- |
| GPT-6.1 Terra, Medium | PASS: model/list advertises medium; thread/start and thread/resume return gpt-6.1-terra with reasoningEffort medium; two turns completed |
| Persistent conversation | PASS: terminate client/server, launch a fresh process, resume the same thread, recall a random marker |
| Connected tools | PASS: model invoked codex_apps / github.search_installed_repositories_v2 and received private repository metadata |
| Desktop reads same conversation | PASS: desktop read_thread returned the probe messages and response; navigate_to_codex_page opened the same thread |
| Desktop UI → companion continuation | PASS with explicit archive/unarchive handoff: companion recalled the desktop-only user message verbatim; desktop read the completed companion turn |
| Attach to desktop's live server | UNPROVEN: current desktop process uses private stdio; default app-server control socket does not exist |
| Desktop-only host tools | UNPROVEN: connector access does not establish access to desktop navigation, screenshots, browser control or artifact host services |

The evidence supports a companion using its own app-server process with shared persistent history and connected account tools. It does not yet prove safe simultaneous turn ownership between desktop and companion. Sequential handoff also needs an explicit writer-release mechanism; navigating away did not release ownership in this test. Desktop history reads can describe an externally running turn as interrupted/not loaded: don't use that field as the companion's authoritative live status.

## Run

Requires Python 3, the installed `codex` executable and an existing authorised ChatGPT login.

```sh
python3 scripts/rpc.py
python3 scripts/prove.py
```

`rpc.py` discovers models, installed connector state, app directory and recent threads. `prove.py` creates a test conversation, sends a random marker, restarts the runtime, resumes the conversation and asks for marker recall plus a read-only GitHub connector call. Model turns consume account usage. Each run creates a probe thread. The small client rejects server-initiated requests it cannot implement; it does not grant approvals.

Raw discovery, event streams and conversation IDs are stored only in ignored `.local/`. They may contain account metadata and must not be committed. The probe inherits local runtime configuration and is not a production security boundary.

## Finish the desktop round trip

The initial proof conversation is named **notdot bridge proof**. Open it in the desktop app and send a fresh harmless marker, for example `Remember desktop marker: violet-lantern-42. Reply with the marker only.` Wait for completion before resuming from another client.

```sh
python3 scripts/continue.py THREAD_ID
```

The continuation asks Terra Medium to recall the desktop marker without supplying it. Verify the returned answer, then reopen the desktop conversation and check that the companion's new turn appears there. This tests both directions without concurrent writers.

## Architecture decision

Proceed with a local, replaceable app-server adapter, persistent thread IDs, streamed agent events and explicit sequential desktop handoff. Keep voice and memory independent of this adapter. Do not rely on private desktop IPC or scrape authentication material.

OpenAI documents app-server for custom clients, but labels the command and WebSocket transport experimental and unsupported for production workloads. Pin and revalidate the installed runtime before release.

References: [app-server](https://learn.chatgpt.com/docs/app-server), [models](https://learn.chatgpt.com/docs/models).

## Desktop handoff follow-up

The desktop added a completed turn saying `Can you recall this?` and recalled the original probe marker. No fresh marker was present in that turn. The companion could read that desktop history via the desktop read API, but three independent resume attempts failed with JSON-RPC -32600: `thread … already has an active writer`. The last attempt followed navigation to another chat and a 45-second wait. No lock files, database state or desktop processes were modified to bypass ownership.

Conclusion: shared history and desktop consumption are proven; a live bidirectional companion/desktop bridge is not. Next investigate a documented shared-runtime attachment or explicit unload/release operation. A separate companion-owned thread remains viable, with desktop used for review rather than concurrent ownership.

## Successful explicit handoff

A supported lifecycle workaround completed the desktop → companion → desktop round trip on 26 October 2026:

1. Archive the idle proof chat using the desktop `set_thread_archived` operation. This released ownership in the tested runtime.
2. Run `codex unarchive THREAD_ID`.
3. Resume through the companion app-server process using Terra Medium.
4. Complete the turn and close the companion process.
5. Read and reopen the same conversation in the desktop.

Repeated this handoff twice. First, the companion recalled the original marker. For stronger proof of desktop-origin history, a second prompt asked it to quote the earlier desktop user message without including that text. It returned `Can you recall this?` exactly. Desktop read_thread then returned that completed companion turn, and desktop navigation reopened the same thread. Raw evidence remains in ignored `.local/desktop-continuation.json` and `.local/desktop-history-recall.json`.

This is an explicit archive/unarchive handoff, not simultaneous shared-runtime control or an invisible UX. The desktop archive operation was invoked through its first-party tool in this investigation; a standalone third-party companion has not yet been shown to have a supported way to invoke that desktop operation. A user can archive the idle conversation manually before companion takeover. Do not archive an active task for handoff.

The official app-server documentation clarifies that thread/unsubscribe affects only the requesting connection and unloads after **30 minutes** with no subscribers or activity. Our earlier 45-second wait was insufficient to test expiry, and navigating away does not prove the desktop unsubscribed. Calling unsubscribe on a separate companion server cannot release desktop ownership. No documented immediate force-release method was found in the installed protocol schema.

Shared app-server transports (stdio, Unix socket and experimental WebSocket) support custom clients, but the current desktop process exposes no default control socket. `codex app-server proxy` cannot attach here. `codex queue` is documented, but its shared-daemon/remote route is not evidence of access to the current private desktop stdio server. No daemon was started, no remote control enabled, and no private IPC or lock files modified.

Updated conclusion: sequential bidirectional history handoff is proven through archive/unarchive. Seamless live shared-runtime attachment remains unproven. For an initial companion, use a companion-owned thread and an explicit desktop handoff action, with manual desktop archive required to hand control back until a supported desktop attachment endpoint exists.

## Ownership and viewing follow-up

Additional controlled tests refine the earlier handoff result:

- An independent client successfully invoked thread/archive, thread/unarchive and thread/resume when no other writer held the thread. No desktop-only archive tool is needed in that state.
- In a controlled two-client test, A held the thread. B's baseline resume failed with `already has an active writer`; B's archive and unarchive both failed with the same error. Therefore an external companion cannot use archive/unarchive to evict a desktop owner. The earlier first-party desktop archive worked because it operated through the owning desktop runtime.
- A long-lived companion client held the thread while desktop navigate_to_codex_page and read_thread accessed it. The companion then completed another Terra Medium turn without handoff. Desktop read access did not steal ownership. This demonstrates history/API viewing, not live native UI repaint, UI submission semantics, or desktop capabilities inside the companion. Computer-use restrictions prevent automated native UI verification.

Preferred product mode: notdot remains the single writer. Desktop is optional history viewing. Do not automatically archive on writer-lock errors; surface ownership and request a supported release instead. No seamless reclaim from desktop is currently proven. Current tests must not be described as live shared-runtime control.

Evidence: ignored `.local/cross-owner.json`, `.local/viewer-events.json`, `.local/ownership.json`. Repeatable viewer and lifecycle probes are in scripts/viewer.py and scripts/ownership.py. The lifecycle probe changes archive state and is intended only for the disposable proof conversation.

## Intended product stack and UI

Current implementation is Python 3 standard-library probe code driving the installed Codex app-server over JSONL stdio. There is no production UI or voice implementation yet.

The intended product can be predominantly Rust:

- Tauri 2 desktop shell and Rust backend; Tokio for process/stream coordination, Serde for protocol messages, SQLite for settings and memory, OS keychain for credentials.
- Rust owns agent lifecycle, single-writer state, voice transport, audio coordination, memory and persistence.
- Small canvas frontend extracted from Lumina's existing Canvas 2D renderer. A thin TypeScript renderer is the lowest-risk visual port; a Rust/WASM renderer is feasible if Rust throughout the UI is preferred. Tauri itself uses a system webview, so a WASM route still needs HTML/CSS and minimal browser/IPC glue.
- Normal interface: only a draggable orb, in a transparent borderless window, with no Lumina panels, dashboard, status cards or composer. Show listening/thinking/speaking/error state through orb animation. Onboarding/settings appear separately on demand through a tray/context menu or shortcut.

Rust/WASM rendering is an architectural option, not implemented or benchmarked. No Tauri scaffold or orb was built during bridge investigation. macOS transparent-webview requirements and distribution implications must be validated against the selected Tauri release before packaging.
