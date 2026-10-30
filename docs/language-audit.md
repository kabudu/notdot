# Language review — 27 October 2026

The app should explain what a setting does before explaining how it works.

## Changes

- Reviewed the full Settings page, its memory and usage messages, the website and 17 first-party Markdown documents.
- Replaced “ledger saving and recall” with saving and using memories. Removed the repeated paragraph saying memory was off.
- Replaced “voice backend” with voice service, “reasoning effort” with thinking level in controls, and “Review & compact” with Tidy memories.
- Explained provisional memories as notes that need confirmation, and showed where notes came from in ordinary language.
- Separated usage figures from billing details. A failed balance check no longer says a speech-enabled key is invalid when it only lacks account access.
- Added a short [Settings guide](settings.md). Kept exact API names, commands and limits in the [developer guide](guide.md), with a glossary.
- Added plain introductions to technical checklists and investigations. Split long prose paragraphs into shorter ones while preserving dated test results.

## Consistent wording

| Wording in the app | Meaning |
| --- | --- |
| Voice service | The service that listens to a request and speaks the reply. |
| Thinking level | How much reasoning the selected model is asked to do. |
| Saved memory | A detail kept separately from the chat history. |
| Needs confirmation | A possible memory that is not yet used in replies. |
| Tidy memories | Review notes and make shorter summaries. |
| Quiet suggestion | A check-in shown on the orb without speaking. |
| Last known balance | An older usage result that could not be refreshed. |

Legal licence texts and upstream third-party notices retain their original wording. Technical names in source code, APIs, logs and dated test evidence stay precise; the user guide explains their meaning. Text supplied by the model catalogue is shown as supplied, rather than rewritten into promises about speed or capability.

## Checks

The app keeps native disclosure and radio controls for keyboard use. Section headings make the choices easy to scan. Detailed billing and testing information is available without filling the main Settings view. Validation commands and native inspection results are recorded with the implementation changes; this language review is not a claim that every screen-reader or platform test is complete.
