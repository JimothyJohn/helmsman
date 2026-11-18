# agent-relay protocol, version 2

Protocol v2 serves the profiles that carry a `harness` block (today
`helmsman/v8`). The relay keeps the conversation: a client opens a
session, sends only new input on each step, runs the tools the relay hands
back, and sends their results. Why and how: `docs/harness.md`.

Everything in protocol v1 (`docs/relay-protocol.md`) still holds: the same
transport, `Authorization: Bearer hmr1_...` token, error shape, body limit,
`GET /v1/health`, `GET /v1/agents` (each profile now says `"protocol": 1`
or `2`) and usage metering. v1 is frozen; a v2 profile is refused on
`POST /v1/agents/{agent}/messages` (400) and a v1 profile on
`POST /v2/sessions` (400).

New errors:

| Status | `type` | `retryable` | When |
|---|---|---|---|
| 409 | `conflict` | false | the input does not fit the session's state; the message says what it is waiting for |
| 409 | `session_busy` | true | another step of the session is running; retry when it ends |

Ids: a session is `s` + 32 lowercase hex, a document `d` + 16 lowercase
hex. Any other id, and another client's id, answers 404 exactly like an id
that does not exist.

## Sessions

### `POST /v2/sessions`

`{"agent": "helmsman/v8"}` (an id or alias). 201:

```json
{"session_id": "s…", "agent": "helmsman/v8", "protocol": 2,
 "harness": {"regime": "eocc", "compact_at_tokens": 40000, …}, "expires_ms": 1786…}
```

A session expires 7 days after its last step.

### `GET /v2/sessions/{id}`

The session's counters and where it stands:

```json
{"id": "s…", "agent": "helmsman/v8", "created_ms": …, "updated_ms": …, "last_seq": 6,
 "model_calls": 3, "compactions": 0, "peak_context_tokens": 9420, "expires_ms": …,
 "phase": {"state": "awaiting_tools", "tool_uses": [{"id": "toolu_…", "name": "read_io_map", "input": {}}]}}
```

`phase.state`: `empty` (no input yet), `done` (the model finished;
`stop_reason`), `awaiting_tools` (the model called tools; `tool_uses`, which
may include relay tools the relay runs with the next step), `ready` (input
is waiting for the model: send `continue`).

### `GET /v2/sessions/{id}/events?after=N`

The log after sequence number `N` (default 0), at most 100 entries, and
`next_after` when there are more. Each entry is `{"seq", "at_ms", "type",
...}` with `type` one of `user`, `assistant`, `tool_results`, `compaction`
(`recap`, `through`), `attempt` (`error`). For audit and debugging; a
client does not need it to run a conversation.

### `DELETE /v2/sessions/{id}`

204. The session, its log and its blobs are gone at once.

## Steps

### `POST /v2/sessions/{id}/steps`

Body: `{"input": I}`, where `I` is one of

- `{"type": "user", "content": "text" | [blocks]}`: a person's message.
  Blocks as in v1 user turns (text, base64 images, base64 PDFs), no
  `tool_result`; at most 20 images and 5 PDFs per input. Accepted when the
  session is `empty`, `done` or `ready`.
- `{"type": "tool_results", "results": [{"tool_use_id": "…", "content": "…" | [blocks], "is_error": false}]}`:
  exactly one result for each waiting client tool call, nothing else (400
  names the ids awaited). The relay runs the response's relay tools and
  appends all results in `tool_use` order.
- `{"type": "continue"}`: resume a session that is `ready` (a step that
  stopped at its budget, or after a failed model call) or whose waiting
  calls are all relay tools.

Only one step runs at a time (409 `session_busy`). Admission happens before
the response starts, with ordinary status codes; once the response is 200,
it is an event stream (`text/event-stream`):

| Event | Data |
|---|---|
| `harness_session` | `{"session_id", "seq"}` first |
| `harness_call` | `{"n", "purpose": "turn" \| "recap"}` before each model call |
| Messages API events | `message_start` … `message_stop` of each `turn` call, as v1 forwards them (a `recap` call is not forwarded) |
| `relay_usage` | after each model call, as v1, plus `purpose` |
| `harness_tool` | `{"tool_use_id", "name", "is_error", "ran"}` for each relay tool the relay ran (or answered unrun) |
| `harness_document` | `{"doc": {…}}` when `fetch_document` stored a document |
| `harness_compaction` | `{"through", "context_tokens", "recap"}` after the context was compacted |
| `harness_yield` | `{"reason", "seq", "tool_uses", "context_tokens", "model_calls"}` last |
| `relay_error` | v1's error body, instead of `harness_yield`, when the step failed |

`harness_yield.reason`:

- `tool_use`: run `tool_uses` (client tools only) and send their results.
- `end_turn`, `max_tokens`, `refusal`, `stop_sequence`: the model finished;
  wait for the person. A response cut off with tool calls in it has those
  calls answered unrun by the relay.
- `continue`: the step spent its budget (model calls or time); send
  `continue` to go on.

After `relay_error` the log is still consistent: a failed model call is an
`attempt` entry (never shown to the model). Retry with `continue` when the
error is `retryable`.

A client that disconnects mid-step does not stop it; the step still
reaches a consistent log, and `GET /v2/sessions/{id}` shows the phase.

## Documents

Stored per client, shared by its sessions, kept 7 days from when they are
stored. The same bytes stored twice are one document (the id comes from
the SHA-256).

### `POST /v2/docs`

The raw bytes as the body, `Content-Type: application/pdf` or
`text/plain` (an EDS is `text/plain` with a name ending `.eds`), and
`x-document-name: <1 to 200 printable characters>`. At most the body limit
(4 MiB), 100 documents per client. 201 with the document:

```json
{"id": "d…", "name": "E1212 manual.pdf", "kind": "pdf", "media_type": "application/pdf",
 "bytes": 2048000, "sha256": "…", "source": "upload", "lines": 4210, "pages": 92,
 "created_ms": …, "expires_ms": …}
```

A PDF without extractable text (a scan) is refused (400): attach it to a
user message instead, where the model reads the pages.

### `GET /v2/docs`, `GET /v2/docs/{id}`, `GET /v2/docs/{id}/raw`, `DELETE /v2/docs/{id}`

The list (`{"docs": [...]}`, oldest first), one document's facts, its
original bytes (headers `content-type`, `x-document-name`,
`x-document-sha256`, `x-document-kind`), and deletion (204).

## Relay-run tools

Tools the relay runs inside a step; their results are data in a
`<relay-data tool="…">` envelope with `<` and `>` escaped:

| Tool | Input (every field required) |
|---|---|
| `doc_list` | `{}` |
| `doc_grep` | `{"doc", "pattern", "max_results"}`: literal, case-insensitive, `\|` separates alternatives (at most 200 bytes, 12 alternatives), 1 to 50 hits |
| `doc_read` | `{"doc", "start_line", "lines"}`: from line 1, 1 to 400 lines, at most 8,000 characters |
| `history_search` | `{"query", "max_results"}` over every log entry, compacted ones included |
| `history_read` | `{"seq", "start", "chars"}`: a byte window, at most 8,000 characters |
| `fetch_document` | as v1; the result is the stored document's id and facts, not the document |

A response may run at most 16 relay tools; the rest are answered unrun.
