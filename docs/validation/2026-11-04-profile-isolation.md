# Member chat isolation

A live local probe passed on macOS using the installed Codex runtime, GPT-6.1 Terra and Medium effort. It used a disposable member profile, a synthetic private file and a synthetic companion chat. It did not request voice keys or read personal chat messages. The synthetic chat was archived afterwards.

## Checks

- The thread-scoped MCP inventory exposed only the bundled `notdot_chats` tools. Inherited apps, MCP services and plugins had no callable tools in this member thread.
- When asked to read a synthetic local file, the model reported that no file-reading tool was available. The random file marker did not appear in its answer.
- The only observed tool calls were two `notdot_chats.find_chats` calls for a deliberately absent synthetic title. No shell, file, browser, app or other tool activity occurred.
- A guessed, ungranted chat ID was rejected at the host policy boundary.
- The turn completed successfully. The companion chat was archived and its temporary roster and file were removed.

The probe checks the installed runtime's real tool inventory and observed behaviour. A single adversarial turn cannot prove that every possible future runtime or prompt is isolated; the deterministic policy tests cover guessed IDs, read-only grants, revocation and filtered discovery separately.

## Reproduce

```sh
cargo run -p notdot-core --locked --example profile_isolation_probe -- /absolute/path/to/codex
```

The example also serves its own ACL-aware MCP helper. It limits the model turn to 120 seconds and archives its synthetic chat before returning the result. The file marker and temporary profile are generated afresh each run.

## Protocol evidence

`codex app-server generate-json-schema --experimental` confirms that `mcpServerStatus/list` accepts `threadId`, which selects the thread's MCP connections, and returns each server's tool inventory. `codex features list` confirms the installed runtime's names for the disabled shell, app, browser, computer, plugin and code execution features. The deny configuration follows the existing analysis runtime configuration, with only the host-owned ACL helper enabled afterwards.

The local machine-readable result is retained in `.local/profile-isolation-live.log`; temporary generated protocol schemas are in `.local/profile-isolation-schema/`. These local files are not part of the distribution.

## Owner and shared-chat checks

The live isolation probe also passed with the owner as the speaker while a member was configured. The companion exposed only its own tools, could not read the synthetic private file, and denied access to the member's private chat. A single owner in open mode keeps the existing tool behaviour. Adding the first member or changing voice access mode revokes old runtime authority so a previously unrestricted connection cannot continue under the new privacy settings.

```sh
cargo run -p notdot-core --locked --example profile_isolation_probe -- /absolute/path/to/codex --owner
cargo run -p notdot-core --locked --example member_posting_probe -- /absolute/path/to/codex
```

The second probe passed through the actual shared-chat runtime. A permitted member's message was appended as a **user** item in the selected chat's saved model context. No model turn started and no completion report was created. The original saved permission history remained exactly the same (`workspace-write` and `untrusted` in this synthetic fixture). Fresh-resume effective permissions also matched the fresh-resume baseline before posting.

This post is context for the chat's next continuation; it does not appear as a separate completed turn in the desktop chat view. The acknowledgement states that no task was started. Member messages cannot steer or stop device tasks. The owner can still explicitly delegate work through eligible Codex chats.

The protocol investigation found that permission overrides supplied to `thread/resume` can persist after release. Member posting therefore supplies **no** permission or tool overrides and never starts a model turn. `thread/inject_items` requires a loaded thread, so posting acquires an idle writer and releases it afterwards. A foreign active writer is rejected rather than interrupted. The current generated protocol describes injected items as model-visible history; it does not promise a rendered desktop message or completed turn.

Machine-readable local evidence: `.local/profile-owner-isolation-live.log` and `.local/member-posting-live.log`. The synthetic target and companion chats were archived after each successful probe.
