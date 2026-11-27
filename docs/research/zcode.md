# ZCode (Z.ai) as a Commutator harness: research notes

Status, 2026-11-27: paused. A working driver over `zcode app-server --stdio`
was built and verified live on branch `zcode-harness` (last commit
`0087b8b`, "Match the ZCode driver to its live wire, replay recorded turns";
three commits before it: `9ce22e0`, `40a694c`, `d8f1344`). The branch may be
gone; the commits are what to look for. Z.ai coding-plan accounts do not work
through it (see "Accounts"); personal providers do.

Sources read: ZCode repo `zai-org/ZCode` at `a4b44451` (2026-11-23, CLI 0.18.0)
and the desktop app 3.14.0 (CLI 0.16.9), download
`https://cdn-zcode.z.ai/zcode/electron/releases/<ver>/macos-arm64/ZCode-<ver>-mac-arm64.dmg`.
Everything below about ZCode's internals is its internal format, checked on
0.16.9 and 0.18.0 only.

## Where the CLI is

- Standalone: no official installer URL is published (the README's
  `ZCODE_DIST_BASE_URL` is a placeholder; `zcode.z.ai/install.sh` is 404). A
  source build (`pnpm build:zcode`) installs `~/.zcode/runtime/releases/<v>`
  and `~/.local/bin/zcode` (`exec node …/bin/zcode.mjs`, needs Node on PATH).
  On macOS that build drops the optional `@napi-rs/keyring-darwin-arm64`
  binding, and `zcode login` fails with "System credential storage is
  unavailable".
- Desktop: `ZCode.app/Contents/Resources/glm/zcode.cjs`, run by the app's own
  Electron: `ELECTRON_RUN_AS_NODE=1 ZCode.app/Contents/MacOS/ZCode zcode.cjs …`.
  It needs `ZCODE_BUILTIN_PROVIDER_CONFIG_FILE=ZCode.app/Contents/Resources/config/provider/zcode-builtin.json`
  (the CLI otherwise looks next to `zcode.cjs`); given only that, it derives the
  rest as the standalone CLI does (`provider-runtime-env.ts`).
- Version: `zcode version` prints the CLI version; `zcode --version` of the
  standalone distribution prints the package version (4.0.0 for 0.18.0).

## app-server wire (`zcode app-server --stdio`)

- Newline JSON, JSON-RPC shaped but with no `jsonrpc` field: the server's zod
  schemas are `.strict()` and refuse it. Our ids are integers; the server's own
  requests use string ids (`"server-1"`). No `initialize`; `runtime/capabilities`
  carries no version.
- Session: `session/create {workspace:{workspacePath, workspaceKey}, mode, model?,
  thoughtLevel?, persistence?, mcpServers?}` → snapshot (`session.sessionId`,
  `settings.model.available`, `slashCommands`); `session/subscribe
  {deliveryKind:"desktop-continuous"}`; `session/send {sessionId, content}`;
  `session/stop`; `session/resume`; `session/setModel {model:{providerId,
  modelId, options:{reasoningLevel}}}`; `session/setMode`; `session/close`.
  `persistence:"deferred"` + close leaves nothing in history (model catalog).
- A send while a turn runs is refused (`-32010` "A prompt is already running"):
  queue mid-turn prompts and send them when the turn ends.
- A selection without a reasoning level fails the turn ("Reasoning level is
  required"); take `reasoning.defaultLevel` from `settings.model.available`.
- Events (`session/event {type, payload, turnId}`): the reply streams as
  `model.streaming` (`text_delta`, `reasoning_delta`, `tool_call` with the full
  input); `part.delta` never came. `tool.updated` `scheduled` has `input:null`;
  `result` is `{success, content, perf:{detail:{command:{exitCode}}}}`; a stop
  ends a running command as `error` (`tool_cancelled`). Turns end with
  `turn.completed {resultType: success|cancelled|error_*}` or `turn.failed`.
- Server → client requests: `session/requestRuntimePreferences` (must be
  answered or create times out after 15 s; strict schema, runtime switches
  only, e.g. `{nativeSearchEnhancementsEnabled:true,
  askUserQuestionAutoResolutionEnabled:false}`), `interaction/requestPermission`
  (answer with the offered option's `response`; options: `allow_once`,
  `allow_always` = rules written to the project, `allow_session` only on the
  v4 path, `deny`; decisions are `allow|deny|escalate|modify`),
  `interaction/requestUserInput` (`{action:"accept", content:{answers:{<question
  text>: answer}, answer_<i>: answer}}`, arrays for multi-select; an empty
  `answers` object means "continue without answers"), and others (provider
  headers, browser, automation, reply language…) where `-32601` makes the
  server fall back as for an older host.
- Modes: `plan`, `build` (asks before risky/side-effecting tools), `edit`,
  `yolo`, `guarded`, `auto` (unimplemented).

## Accounts (why Z.ai plans don't work over app-server)

- The app-server builds its provider registry without the standalone account
  layer (`zcode-protocol-entrypoint.ts` → `startProcessProviderRegistryRuntime`
  without `standalone`). `-p` and the TUI pass it; there the CLI decrypts its
  credentials, resolves the plan and signs requests itself.
- Over app-server the host must push the plan (`provider/updateAccountConfig`,
  then `provider/refreshRegistry`) and answer
  `interaction/requestProviderRuntimeHeaders`, as the desktop's
  `services/src/zcode-agent/zcodeAgentService.ts` does. Without it: "Provider
  Registry 中不存在 Model: account:zai-individual-coding-plan/GLM-5.3".
- Personal providers (`~/.zcode/v2/provider_config.json` `providerRules`,
  `group:"standard-personal"`, `access:{type:"api-key", apiKey}`,
  `api:{type:"openai-chat-completions", baseUrl}`, `personalModelIds`) need no
  host and work. `ZCODE_PERSONAL_PROVIDER_CONFIG_FILE` points the CLI at
  another file (used for tests).

### If the account layer is ever reimplemented ("option C")

- `~/.zcode/v2/credentials.json`: `{key: "enc:v2:os:<sha256(key)[..24] hex>:<iv>.<tag>.<ct>"}`,
  base64url parts, AES-256-GCM, IV 12, tag 16
  (`packages/shared/src/credential-security/systemCredentialCipher.ts`).
- Key: Keychain generic password, service `ZCode Credential Encryption`,
  account = hex sha256 of the absolute credentials.json path, value = base64url
  of 32 bytes; used when `credential-source` is `os\n`. Otherwise
  `ZCODE_CREDENTIAL_SECRET(_FILE)` or a 0600 `credential-secret` beside it,
  key = sha256(secret). Reading it from another app makes macOS prompt.
- Plan snapshot (`standalone-account-provider-runtime.ts`): built-in providers
  with `access {type:"zhipu-account", mode:"individual-coding-plan"}`; entitled
  when `account-provider:<id>:identity` and the OAuth token
  (`oauth:zai:access_token`, or BigModel's) / manual key are present.
  `providers = {<id>: {access:{type:"zhipu-account", entitled}}}`,
  `states[<id>] = {availability, entitled, current}` (`current` required when
  entitled), `basedOnZCodeBuiltinRevision` must equal the worker's
  (`zcode-builtin:<release.revision>:<sourceKey>`) or the registry ignores it,
  `revision = "account:" + JSON([base, providers, states])`.
- Request auth: the OAuth token is exchanged for a project API key
  (`adapters/src/auth/coding-plan-api-key.ts`); the original caches its
  location in the credential store, which must not be written; token refresh
  not studied.

## How others do it

- Orca (`stablyai/orca`): ZCode as a terminal agent (its TUI in a pane, plus
  hooks, status, resume); the plan works because the TUI owns it. No chat UI.
- T3 Code (`pingdotgg/t3code`): no ZCode.
- zcode-acp (`supermomonga/zcode-acp`, unofficial): runs the desktop's host
  service headless (Electron in node mode) and speaks its private NDJSON
  `zcode-task-v1`, exposing ACP. Plans work with no credential handling, but
  it is pinned to exact desktop builds (3.11.2 / CLI 0.16.5, hash-checked) and
  relies on an undocumented mode. The likely cheaper route to plan support than
  option C: study its docs (`04-zcode-private-protocol.md`) and source.

## Live facts from the paused work

- The test account had no balance: `zcode -p` → `[1113] Insufficient balance
  or no resource package` (HTTP 429), the expected answer once auth works.
- Driver verified live (personal vLLM provider, 0.18.0 and 0.16.9): catalog,
  supervised Bash turn with a permission, resume in a new process, stop mid
  command. Not verified: a prompt sent mid-turn, the Commutator app UI.
