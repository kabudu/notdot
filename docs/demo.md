# Real voice demo — production checklist

Status: **not recorded**. Native access and saved-key authorisation are pending. Do not replace this requirement with generated speech over a staged animation.

Record the actual QA app, with staged non-sensitive content, microphone input and audible app output. Keep all keys, private windows, account names and history out of frame. Suggested 45–75 second sequence:

1. Show the real orb; say “What is three times seven?” and keep the audible reply.
2. With learning enabled, say “Remember that I prefer concise answers.” Show the actual acknowledgement.
3. Ask a follow-up in the same conversation; show listening, thinking and speaking states.
4. Briefly show the colour preview and voice picker, without credential fields or private notes.

Do not hide failure or cut away waiting time to imply unmeasured latency. Label any time compression. Listen to the exported file end to end; verify both sides are audible and no private content appears. Obtain consent from anyone whose voice is recorded.

## Publication assets

Place the verified H.264/AAC MP4, actual-frame poster, WebVTT captions and plain transcript under `docs/site-assets/demo/`. Use `<video controls playsinline preload="metadata" poster="…">` plus a caption track on the website; no autoplay. Provide a transcript link. Do not add broken references before these files exist.

GitHub Markdown sanitises arbitrary video HTML. Upload the actual MP4 through GitHub’s attachment UI and verify its rendered player in the README; retain a clickable poster/direct file fallback. Do not claim an inline player works until it has been checked in the rendered README. Keep the repository copy as the durable downloadable asset. Remove this pending status from the README/site only after actual media verification.
