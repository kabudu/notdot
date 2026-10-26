# notdot

Private feasibility prototype for a desktop voice companion. No voice, orb, memory ledger or onboarding has been built yet.

## Bridge proof — 26 October 2026

Tested against the Codex runtime bundled with ChatGPT Desktop, `codex-cli 0.164.0-alpha.11.1`, using the existing local ChatGPT sign-in. No API key was copied or required.

| Requirement | Result |
| --- | --- |
| GPT-6.1 Terra, Medium | PASS: model/list advertises medium; thread/start and thread/resume return gpt-6.1-terra with reasoningEffort medium; two turns completed |
| Persistent conversation | PASS: terminate client/server, launch a fresh process, resume the same thread, recall a random marker |
| Connected tools | PASS: model invoked codex_apps / github.search_installed_repositories_v2 and received private repository metadata |
| Desktop reads same conversation | PASS: desktop read_thread returned the probe messages and response; navigate_to_codex_page opened the same thread |
| Desktop UI → companion continuation | PENDING: native computer-use tool rejects com.openai.codex; needs manual UI turn followed by probe resume |
| Attach to desktop's live server | UNPROVEN: current desktop process uses private stdio; default app-server control socket does not exist |
| Desktop-only host tools | UNPROVEN: connector access does not establish access to desktop navigation, screenshots, browser control or artifact host services |

The evidence supports a companion using its own app-server process with shared persistent history and connected account tools. It does not yet prove safe simultaneous turn ownership between desktop and companion. Use sequential handoff until concurrency is tested. Desktop history reads can describe an externally running turn as interrupted/not loaded: don't use that field as the companion's authoritative live status.

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
