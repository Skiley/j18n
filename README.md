# j18n

LLM-powered CLI for syncing translated JSON locale files. Point it at a
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
- **Pluggable backends** — Claude Code (the local `claude` CLI) or the Gemini
  HTTP API. Adding another is a small trait impl.
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
- **Polished output** — auto-detects existing indentation (tab / 2-space /
  4-space) per file, sorts keys in natural order (numbers as numbers,
  case-insensitive with sensible camelCase tiebreaker), preserves your
  reference file untouched.
- **Cross-platform stable cache** — cache identifiers come from your config
  strings, not resolved file paths. A cache generated on Windows works on
  Linux/macOS, and vice versa.

## Quick start

Build:

```sh
cargo build --release -p j18n-cli
```

Generate a config:

```sh
./target/release/j18n init my-project.json
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
    "translator": "claude-code"
}
```

Sync:

```sh
./target/release/j18n sync my-project.json
```

`pt.json` and `es.json` now contain translations of every key in `en.json`.
Run again at any time — only entries whose `en.json` value changed (or that
are missing in the target) are re-translated.

## Commands

```
j18n init        <PATH>          # write a skeleton config to <PATH>
j18n sync        <CONFIG>...     # translate missing or changed entries
j18n regenerate  <CONFIG>...     # re-translate every entry, replacing existing values
```

Each command accepts one or more configs and processes them in order.

## Configuration

| Field                    | Type                | Description |
| ------------------------ | ------------------- | ----------- |
| `additionalPrompts`      | string[]            | Extra prompt lines — domain context, glossary rules — inserted between the placeholder warnings in the LLM prompt. |
| `batchSize`              | integer (≥ 1)       | Entries per LLM call. `init` default: 50. |
| `excludePatterns`        | string[]            | Glob patterns of dot-separated keys to skip. See **Patterns**. |
| `generateI18nFor`        | object[]            | Target locales: `{ "file": "...", "language": "..." }`. |
| `hashCacheLocation`      | string *(optional)* | Override where the cache lives. Defaults to `.hash-cache.json` in the reference file's directory. |
| `interpolationPatterns`  | string[]            | Regexes matching substrings to preserve verbatim through translation. See **Patterns**. |
| `parallelBatches`        | integer (≥ 1)       | Max LLM batches in flight. `init` default: 3. |
| `referenceI18n`          | object              | Source locale, same shape as a target. |
| `translator`             | enum                | `"claude-code"` or `"gemini-api"`. |

Paths in `referenceI18n.file`, `generateI18nFor[].file`, and
`hashCacheLocation` resolve relative to the directory of the config file.
Absolute paths pass through unchanged.

`language` is whatever string you want the LLM to see — there's no fixed list,
no ISO-639 lookup. Write the phrasing you want.

## Backends

### `claude-code`

Spawns the local `claude` CLI (`cmd /C claude --model=opus -p` on Windows,
`claude --model=opus -p` elsewhere). Make sure `claude` is on `PATH`.

### `gemini-api`

Calls Gemini's `generateContent` HTTP endpoint. Requires `GEMINI_API_KEY` in
the environment; fails fast at startup if missing.

```sh
GEMINI_API_KEY=... j18n sync my-project.json
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
- **Key order** is natural:
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
