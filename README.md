# j18n

LLM-powered CLI for syncing translated locale files. Point it at a
reference file (typically `en.json`), list your target languages, and it fills
in the rest — incrementally, re-translating only entries whose source actually
changed.

## Why j18n

Most i18n tooling is either:

- **A key manager** (Lokalise, Crowdin, Phrase) — great for collaboration, but
  you're paying per seat and uploading your strings to a SaaS, and the actual
  translation still happens in a separate workflow; or
- **A bulk machine-translation pipeline** — fast and cheap, but the output
  reads like machine translation, ignores domain context, and clobbers
  placeholders.

j18n sits in between: a small open-source binary you point at your locale
files, with the translation handled by an LLM that you choose (the local
`claude` CLI or the Gemini API). You stay in control of prompts, glossary
rules, file layout, and what gets re-translated when.

## Features

- **Incremental sync** — per-target hash cache only re-translates entries
  whose source actually changed since that locale was last synced. Sync
  failures don't lose progress: completed locales keep their fresh cache
  entries.
- **Namespace support** — point one config at an i18next-style
  `locales/{lang}/{namespace}.json` layout (or any layout with a
  `{namespace}` token in the path) and j18n handles every namespace in one
  run. Namespaces can be listed explicitly or auto-discovered with `"*"`.
- **JSON or Markdown** — translate i18n JSON dictionaries or whole
  Markdown/MDX documents (`"format": "markdown"`), the latter preserving all
  Markdown/MDX syntax and front-matter keys while only re-translating a
  document when its source changes. See **Formats**.
- **Pluggable backends** — Claude Code (the local `claude` CLI), Codex CLI (the
  local `codex` CLI), or a direct HTTP API: Gemini, Anthropic, OpenAI, or
  OpenRouter. Each backend lets you pick the model, and the CLI-based ones also
  let you pick a reasoning effort level. Adding another is a small trait impl.
- **Free-form language names** — write `"Brazilian Portuguese"` or
  `"Simplified Chinese (Taiwan-style punctuation)"` and that's literally what
  the LLM sees. No hardcoded language list to limit you.
- **Placeholder-safe** — substrings matching your interpolation regex(es)
  (`{{name}}`, `{0}`, `%count%`, ...) are extracted to neutral `[N]` markers
  before the prompt and spliced back after, so the LLM can't drop or mangle
  them.
- **Key exclusion** — skip dev/sample/internal keys with dot-glob patterns
  (`sample.**`, `*.debug`).
- **Domain prompting** — append your own glossary rules
  (`"don't translate 'playlist'"`, `"context is a music app"`) without
  forking.
- **Order-preserving output** — auto-detects existing indentation (tab /
  2-space / 4-space) per file and keeps each target file's existing key order
  intact. Existing keys are never reordered, so any hand-made ordering survives;
  only keys a target doesn't have yet are inserted in natural order (numbers as
  numbers, case-insensitive with a sensible camelCase tiebreaker). Your
  reference file is left untouched.
- **Cross-platform stable cache** — cache identifiers come from your config
  strings, not resolved file paths. A cache generated on Windows works on
  Linux/macOS, and vice versa.

## Install

npm:

```bash
npm install -g @j18n/cli
```

Linux / macOS:

```bash
curl -fsSL https://github.com/Skiley/j18n/releases/latest/download/install.sh | sh
```

Windows (via PowerShell):

```powershell
iwr https://github.com/Skiley/j18n/releases/latest/download/install.ps1 | iex
```

Or build from source (see [Building from source](#building-from-source)).

## Quick start

Generate a config:

```sh
j18n init
```

Edit it to point at your locales:

```json
{
    "additionalPrompts": [],
    "batchSize": 50,
    "excludePatterns": [],
    "generateI18nFor": [
        { "file": "locales/pt.json", "language": "Portuguese" },
        { "file": "locales/es.json", "language": "Spanish" }
    ],
    "interpolationPatterns": ["\\{\\{(.+?)\\}\\}"],
    "parallelBatches": 3,
    "referenceI18n": { "file": "locales/en.json", "language": "English" },
    "retriesPerError": 3,
    "translator": "claude-code"
}
```

Sync:

```sh
j18n sync
```

`pt.json` and `es.json` now contain translations of every key in `en.json`.
Run again at any time — only entries whose `en.json` value changed (or that
are missing in the target) are re-translated.

By default, the file name is `j18n.json`. You can change that by specifying `-f name.json`.

## Commands

```
j18n init              [-f <PATH>]     # write a skeleton config (defaults to j18n.json)
j18n sync              [-f <PATH>...]  # translate missing or changed entries
j18n regenerate        [-f <PATH>...]  # re-translate every entry, replacing existing values
j18n check             [-f <PATH>...]  # dry-run sync; exits non-zero if anything would change
j18n baseline          [-f <PATH>...]  # record current reference hashes without translating; use when adopting j18n on a project that already has translations
j18n install-git-hook <HOOK> [-f <PATH>...]  # install the given git hook (e.g. pre-commit, pre-push) that runs `j18n check`
```

Every command takes its config path via `-f`/`--file` and defaults to
`j18n.json` in the current directory when omitted. For commands that read a
config (everything except `init`), pass `-f` multiple times to act on
several configs in one run (e.g. `j18n check -f web.json -f mobile.json`).
`check` is meant for CI pipelines; it exits with a non-zero status if any
target locale is out of sync (missing keys, stale keys, or changed reference
values). `install-git-hook` takes a required hook name and writes
`.git/hooks/<HOOK>` so the chosen action fails until you run `j18n sync` (e.g.
`j18n install-git-hook pre-commit` blocks commits, `j18n install-git-hook
pre-push` blocks pushes).

`baseline` writes (or merges into) the hash cache file from the **current**
reference and target file contents, marking each existing target translation
as in-sync. It does not call the LLM and does not modify any locale files.
Use it once when you start using j18n on a project that already has
hand-translated files — otherwise the first `sync` would re-translate
everything because the cache starts empty.

Per target, baseline only records hashes for reference keys that **also exist
in the target file**. Reference keys missing from a target are deliberately
left out so a follow-up `sync` translates them (and only them) — partial
translations are handled correctly. Existing cache entries for targets not
touched by this baseline (e.g. from another config sharing the same cache
file) are preserved.

## Continuous integration

Use the bundled GitHub Action to install j18n on a runner, then run `j18n
check` to fail the build when any locale is out of sync. `check` is a dry run —
it compares hashes only and never calls the LLM, so it needs no API key:

```yaml
name: i18n
on: [pull_request]

jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: Skiley/j18n/.github/actions/setup@master
      - run: j18n check
```

The action downloads the release binary matching the runner's OS/arch
(Linux/macOS/Windows, x64/arm64) and adds it to `PATH`. Pin `version:` to a
specific release if you don't want `latest`:

```yaml
      - uses: Skiley/j18n/.github/actions/setup@master
        with:
          version: v0.2.0
```

## Configuration

| Field                    | Type                | Description |
| ------------------------ | ------------------- | ----------- |
| `additionalPrompts`      | string[]            | Extra prompt lines — domain context, glossary rules — inserted between the placeholder warnings in the LLM prompt. |
| `batchSize`              | integer (≥ 1)       | Entries per LLM call. `init` default: 50. |
| `excludePatterns`        | string[]            | Glob patterns of dot-separated keys to skip. See **Patterns**. |
| `format`                 | string *(optional)* | `"json"` (default) or `"markdown"`. Picks how files are parsed and written. See **Formats**. |
| `generateI18nFor`        | object[]            | Target locales: `{ "file": "...", "language": "..." }`. |
| `hashCacheLocation`      | string *(optional)* | Override where the cache file lives. Defaults to `.j18n-cache.ini` in the reference file's directory (or the deepest fixed-prefix directory when using namespaces). |
| `interpolationPatterns`  | string[]            | Regexes matching substrings to preserve verbatim through translation. See **Patterns**. |
| `namespaces`             | string \| string[] *(optional)* | `"*"` to auto-discover namespaces in the reference's directory, `"**"` to discover recursively (nested `{namespace}` paths), or an explicit list. Required when any `file` contains `{namespace}`; forbidden otherwise. See **Namespaces**. |
| `parallelBatches`        | integer (≥ 1)       | Max LLM batches in flight. `init` default: 3. |
| `referenceI18n`          | object              | Source locale, same shape as a target. |
| `retriesPerError`        | integer (≥ 0)       | How many times to retry a batch when translation fails (LLM HTTP error, mismatched interpolation count, validation failure, etc.). A value of `0` disables retries — the batch fails on the first error. `init` default: 3. |
| `translator`             | string              | `"<kind>[/<model>[/<effort>]]"`. See **Backends**. |

Paths in `referenceI18n.file`, `generateI18nFor[].file`, and
`hashCacheLocation` resolve relative to the directory of the config file.
Absolute paths pass through unchanged.

`language` is whatever string you want the LLM to see — there's no fixed list,
no ISO-639 lookup. Write the phrasing you want.

## Namespaces

For projects that split translations across multiple JSON files per language
(e.g. `locales/{lang}/common.json`, `locales/{lang}/auth.json`,
`locales/{lang}/checkout.json` — the layout `i18next` calls "namespaces"), one
j18n config can drive the whole tree.

Put `{namespace}` somewhere in every `file` path and add a top-level
`namespaces` field. The token expands once per namespace, and j18n runs the
sync for each namespace using the same translator settings, exclude patterns,
and shared hash cache.

Wildcard mode — auto-discover every namespace in the reference's directory:

```json
{
    "additionalPrompts": [],
    "batchSize": 50,
    "excludePatterns": [],
    "generateI18nFor": [
        { "file": "locales/pt/{namespace}.json", "language": "Portuguese" },
        { "file": "locales/es/{namespace}.json", "language": "Spanish" }
    ],
    "interpolationPatterns": ["\\{\\{(.+?)\\}\\}"],
    "namespaces": "*",
    "parallelBatches": 3,
    "referenceI18n": { "file": "locales/en/{namespace}.json", "language": "English" },
    "retriesPerError": 3,
    "translator": "claude-code"
}
```

Drop a new file into `locales/en/` and the next `j18n sync` picks it up
automatically — no config edit.

Recursive wildcard mode — `"**"` walks the reference directory **and every
subdirectory**, so the `{namespace}` token represents a nested relative path.
This collapses a whole tree of source files into one config — handy for
documentation sites (e.g. a Docusaurus `docs/` tree translated with
`"format": "markdown"`):

```json
{
    "additionalPrompts": [],
    "batchSize": 1,
    "excludePatterns": [],
    "format": "markdown",
    "generateI18nFor": [
        { "file": "i18n/pt/docusaurus-plugin-content-docs/current/{namespace}.mdx", "language": "Brazilian Portuguese" }
    ],
    "interpolationPatterns": [],
    "namespaces": "**",
    "parallelBatches": 3,
    "referenceI18n": { "file": "docs/{namespace}.mdx", "language": "English" },
    "retriesPerError": 3,
    "translator": "claude-code"
}
```

Against `docs/welcome.mdx`, `docs/getting-started/faq.mdx`,
`docs/legal/terms-of-use.mdx`, … this discovers `welcome`,
`getting-started/faq`, `legal/terms-of-use`, … — each substituted back into
both the reference and target paths. `"**"` requires the `{namespace}` token to
be in the **file name** (not a directory component).

Explicit list — list namespaces by hand when you want full control over what
gets processed:

```json
"namespaces": ["common", "auth", "checkout"]
```

### Rules

- If `namespaces` is set, **every** `file` (reference and targets) must
  contain `{namespace}` exactly once. Mixed mode is rejected.
- If `namespaces` is not set, no `file` may contain `{namespace}`.
- `{namespace}` can sit in any path component — the basename
  (`locales/en/{namespace}.json`), a directory
  (`features/{namespace}/i18n/en.json`), or with a literal prefix/suffix
  around it (`locales/en/i18n-{namespace}-bundle.json`,
  `features/feat-{namespace}-mod/en.json`).
- Wildcard discovery skips dotfiles and verifies that the **full**
  substituted reference path exists, so directories without the expected
  inner file (and stray dotfiles like `.j18n-cache.ini`) are never
  mistaken for a namespace.
- Files or directories whose names don't match the pattern (e.g. a stray
  `README.md`) are ignored.
- `"**"` (recursive) requires `{namespace}` to be in the **file name** (not a
  directory component), so a nested namespace substitutes back into a valid
  path. Discovered names use `/` separators; hidden directories (starting with
  `.`) are skipped.

### Hash cache location with namespaces

Default: `<deepest-fixed-prefix-dir>/.j18n-cache.ini`, where the "deepest
fixed prefix" is the part of the reference template before the path component
containing `{namespace}`. For `locales/en/{namespace}.json` that's
`locales/en/`. Override with `hashCacheLocation` if you want it elsewhere.

The cache is one INI-style file with one section per `(target, namespace)`
combination, sorted alphabetically by target id, with `key=hash` lines
sorted naturally per section. Sync, check, and baseline each stream the
file line-by-line and only retain the section for the target they're
processing, keeping memory bounded regardless of project size.

```ini
[locales/pt/auth.json@Portuguese]
login.password=4f6a9
login.title=-8a0ced2
[locales/pt/common.json@Portuguese]
button.cancel=61
button.ok=c21
```

Saves rewrite the file via a temp + atomic rename, so an interrupted save
never leaves the cache in a half-written state. Target ids must not
contain `[`, `]`, or newlines; cache keys must not contain `=` or
newlines — j18n validates this at write time.

## Formats

The `format` field selects how each file is parsed into translatable entries
and written back. Everything else — incremental sync, the hash cache, batching,
parallelism, retries, namespaces, exclude/interpolation patterns, and the
backends — works the same regardless of format.

### `json`

The original behavior: a reference JSON object is flattened into dotted-key
entries (`section.button.ok`), each string value translated independently, and
the target is written as pretty-printed JSON that keeps the target file's
existing key order — only keys it doesn't have yet are inserted, each in
natural order. Non-string values are left untouched.

### `markdown`

Each Markdown/MDX file is treated as **one entry whose value is the whole
document**. The target is written verbatim (with a single trailing newline),
not reserialized — there's no flattening, sorting, or pruning. The prompt is
swapped for a document-translation prompt that instructs the model to preserve
all Markdown/MDX syntax (code fences, inline code, URLs, link targets, image
paths, HTML/JSX tags and component names, import/export lines) and front-matter
keys, translating only human-readable prose, headings, link text, and alt text.

Because a file is a single entry, the incremental cache re-translates a document
only when its source content changes, and `batchSize` is effectively one
document per call. Configure `interpolationPatterns` to additionally hard-lock
any substrings you never want touched (they are extracted to neutral `[N]`
markers before the prompt and restored after, with integrity validated).

Pair `markdown` with namespaces to translate a whole docs tree in one run — for
example, feeding a Docusaurus `i18n/<locale>/.../current/` layout:

```jsonc
{
    "additionalPrompts": [],
    "batchSize": 1,
    "excludePatterns": [],
    "format": "markdown",
    "generateI18nFor": [
        { "file": "i18n/pt-BR/docusaurus-plugin-content-docs/current/{namespace}.mdx", "language": "Brazilian Portuguese" }
    ],
    "interpolationPatterns": [],
    "namespaces": "*",
    "parallelBatches": 3,
    "referenceI18n": { "file": "docs/{namespace}.mdx", "language": "English" },
    "retriesPerError": 3,
    "translator": "claude-code"
}
```

## Backends

The `translator` field is a slash-separated string of the form
`"<kind>[/<model>[/<effort>]]"`. Omitted segments fall back to per-backend
defaults.

| Kind            | Format                                | Default model           | Default effort | Notes |
| --------------- | ------------------------------------- | ----------------------- | -------------- | ----- |
| `claude-code`   | `claude-code[/<model>[/<effort>]]`    | `opus`                  | `high`         | Effort is injected as a directive line in the prompt — the CLI itself doesn't have a native effort flag. |
| `gemini-api`    | `gemini-api[/<model>]`                | `gemini-3.1-pro-preview`| (n/a)          | Model name without the `gemini-` prefix is auto-prefixed (so `3.1-pro` → `gemini-3.1-pro`). |
| `codex`         | `codex[/<model>[/<effort>]]`          | `gpt-5.1`               | `high`         | Effort maps to `-c model_reasoning_effort=<effort>` and is also injected into the prompt. |
| `anthropic-api` | `anthropic-api[/<model>]`             | `claude-sonnet-4-5`     | (n/a)          | Direct Anthropic Messages API (not the `claude` CLI). Requires `ANTHROPIC_API_KEY`. |
| `openai-api`    | `openai-api[/<model>]`                | `gpt-5.1`               | (n/a)          | Direct OpenAI Chat Completions API (not the `codex` CLI). Requires `OPENAI_API_KEY`. |
| `openrouter-api`| `openrouter-api[/<model-slug>]`       | `openai/gpt-5.1`        | (n/a)          | OpenAI-compatible gateway to many models. Model slugs contain a `/` (e.g. `anthropic/claude-sonnet-4.5`). Requires `OPENROUTER_API_KEY`. |

Examples:

```jsonc
"translator": "claude-code"                  // opus, high effort
"translator": "claude-code/opus/medium"      // opus, medium effort
"translator": "claude-code/sonnet/low"       // sonnet, low effort
"translator": "gemini-api"                   // default Gemini pro model
"translator": "gemini-api/3.1-pro"           // gemini-3.1-pro
"translator": "gemini-api/gemini-3.1-pro-preview"
"translator": "codex/gpt-5.1"                // gpt-5.1, high effort
"translator": "codex/gpt-5.1/low"            // gpt-5.1, low effort
"translator": "anthropic-api"                // default claude-sonnet-4-5 via API
"translator": "anthropic-api/claude-opus-4-5"
"translator": "openai-api"                   // default gpt-5.1 via API
"translator": "openai-api/gpt-4.1-mini"
"translator": "openrouter-api"               // default openai/gpt-5.1 via OpenRouter
"translator": "openrouter-api/anthropic/claude-sonnet-4.5"
```

### `claude-code`

Spawns the local `claude` CLI (`cmd /C claude --model=<model> -p` on Windows,
`claude --model=<model> -p` elsewhere). Make sure `claude` is on `PATH`.

### `gemini-api`

Calls Gemini's `generateContent` HTTP endpoint. Requires `GEMINI_API_KEY` in
the environment; fails fast at startup if missing.

```sh
GEMINI_API_KEY=... j18n sync my-project.json
```

### `codex`

Spawns the local `codex` CLI in non-interactive mode
(`codex exec --color never --model=<model> -c model_reasoning_effort=<effort> -`)
and feeds the prompt over stdin. Make sure `codex` is on `PATH`.

### `anthropic-api`

Calls Anthropic's `/v1/messages` HTTP endpoint directly (no local CLI). Requires
`ANTHROPIC_API_KEY` in the environment; fails fast at startup if missing.

```sh
ANTHROPIC_API_KEY=... j18n sync my-project.json
```

### `openai-api`

Calls OpenAI's `/v1/chat/completions` HTTP endpoint directly (no local CLI).
Requires `OPENAI_API_KEY`; fails fast at startup if missing. The request is kept
minimal (no `temperature`/`max_tokens`) so both chat and reasoning models work.

```sh
OPENAI_API_KEY=... j18n sync my-project.json
```

### `openrouter-api`

Calls [OpenRouter](https://openrouter.ai), an OpenAI-compatible gateway, at
`https://openrouter.ai/api/v1/chat/completions`. Requires `OPENROUTER_API_KEY`;
fails fast at startup if missing. Because OpenRouter model slugs contain a `/`,
everything after `openrouter-api/` is taken verbatim as the model
(e.g. `openrouter-api/anthropic/claude-sonnet-4.5`).

```sh
OPENROUTER_API_KEY=... j18n sync my-project.json
```

## Patterns

### Exclude patterns

Operate on dot-separated key paths in the source JSON. Within a segment, `*`
matches any run of non-dot characters and `?` matches a single character. `**`
matches any number of components.

| Pattern        | Matches                                  | Doesn't match |
| -------------- | ---------------------------------------- | ------------- |
| `sample`       | `sample`                                 | `sampler`, `sample.foo` |
| `sample.**`    | `sample`, `sample.foo`, `sample.foo.bar` | `other.sample` |
| `*.debug`      | `auth.debug`, `pay.debug`                | `debug`, `auth.flow.debug` |
| `**.todo`      | `todo`, `auth.todo`, `auth.x.todo`       | `todoist` |

Excluded keys are dropped before the LLM ever sees them, and won't appear in
target files either.

### Interpolation patterns

Each regex is applied in order to every value before translation. Matches are
replaced with `[0]`, `[1]`, ... so the LLM can't accidentally translate or
drop them, and the original substrings are restored after.

| Style                | Regex (in JSON)             |
| -------------------- | --------------------------- |
| `{{name}}`           | `"\\{\\{(.+?)\\}\\}"`       |
| `{0}`, `{count}`     | `"\\{[^{}]+\\}"`            |
| `%name%`             | `"%\\w+%"`                  |

Combine multiple styles by adding more entries. Empty array means no
interpolation handling.

### Additional prompts

`additionalPrompts` lines are inserted after the placeholder warning and
before the reminder. Use them for glossary rules and domain context:

```json
"additionalPrompts": [
    "Context: this is a music streaming app.",
    "DO NOT translate the words 'playlist', 'artwork', or 'feedback'.",
    "The word 'track' should be interpreted as 'song'."
]
```

## How sync decides what to translate

For each target locale, on `sync`:

1. Load the per-target hash entry from the cache (empty if first run).
2. Compute current hashes for every reference key.
3. Translate any entry that is **missing in the target file** OR whose
   **reference hash changed** since the last sync of this target.
4. Write the target file.
5. Persist the updated per-target hash entry.

The cache is rewritten after each target completes. If sync fails on target
#19 of 21, targets 1–18 keep their up-to-date entries — they won't be
re-translated on the next run.

`regenerate` ignores the cache and re-translates every entry, then writes the
cache as if everything had been done from scratch.

## Output details

- **Indentation** auto-detects from the existing target file (when syncing) or
  the reference file. Falls back to a single tab.
- **Key order is preserved.** A target file's existing keys are never
  reordered, so hand-made ordering survives `sync` and `regenerate` — values are
  edited in place. Only keys that are new to a target are inserted, each at its
  natural-order position (so a wholly new file comes out fully natural-sorted).
  Natural order:
  - numbers compare as numbers (`"1"`, `"2"`, ..., `"10"`, `"11"`),
  - case-insensitive primary sort (`none` before `noSuggestions`),
  - uppercase wins as tiebreaker when one string is a case-fold prefix of the
    other (`typeSelection` before `types`).
- **Trailing newline** at end of file.
- **Reference file is never written.** Sync only modifies target files and
  the hash cache.

## Logging

Uses `tracing` with `tracing-subscriber`. Override the level with `RUST_LOG`:

```sh
RUST_LOG=debug j18n sync my-project.json
```

Logs go to stderr; stdout is left clean for piping.

## Building from source

```sh
cargo build --release -p j18n-cli
```

The binary lands at `target/release/j18n` (or `j18n.exe` on Windows).

## License

MIT.
