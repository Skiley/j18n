# j18n

A small CLI for generating and syncing localized JSON files from a reference
language using LLM-backed translation. Pluggable backends — currently Claude
Code (the local `claude` CLI) and the Gemini API.

## Layout

The project is a Cargo workspace with one crate per concern:

| Crate              | Purpose                                                                         |
| ------------------ | ------------------------------------------------------------------------------- |
| `j18n-core`        | Shared types: `I18nDefinition`, `I18nData`, `GenerationMode`, `PathPattern`, errors |
| `j18n-io`          | JSON reader/writer, key walker, hash cache, indent detection                    |
| `j18n-translator`  | `I18nTranslator` trait and the placeholder extrapolation/restoration helpers    |
| `j18n-claude-code` | Translator that drives the local `claude` CLI as a subprocess                   |
| `j18n-gemini-api`  | Translator that calls the Gemini `generateContent` HTTP API                     |
| `j18n-validator`   | Sanity checks that translated values keep their interpolations                  |
| `j18n-generator`   | Orchestrator: batches entries, runs translators, writes output, refreshes cache |
| `j18n-cli`         | Binary crate exposing the `j18n` executable                                     |

## CLI

```
j18n init                <PATH>
j18n sync                <CONFIG>...
j18n regenerate          <CONFIG>...
j18n migrate-hash-cache  <CONFIG> <HASH_CACHE>
```

- `init` – write a skeleton JSON configuration file at `<PATH>`. Refuses to
  overwrite an existing file. Creates parent directories as needed.
- `sync` – translate only entries that are missing in the target file or whose
  reference value changed since that target was last synced (tracked
  per-target in `.hash-cache.json` next to the reference file).
- `regenerate` – re-translate every entry in the reference, replacing the
  existing values.
- `migrate-hash-cache` – one-shot migration of an old flat hash-cache file
  into the per-target structure. Temporary; will be removed.

Each positional `<CONFIG>` is a path to a JSON configuration file (see below);
the tool runs the chosen mode against each config in sequence.

## Configuration file schema

```json
{
    "additionalPrompts": [],
    "batchSize": 50,
    "excludePatterns": ["sample.**"],
    "generateI18nFor": [
        { "file": "locales/pt.json", "language": "Brazilian Portuguese" },
        { "file": "locales/es.json", "language": "Spanish" }
    ],
    "interpolationPatterns": ["\\{\\{(.+?)\\}\\}"],
    "parallelBatches": 3,
    "referenceI18n": { "file": "locales/en.json", "language": "English" },
    "translator": "claude-code"
}
```

| Field                    | Type                | Description |
| ------------------------ | ------------------- | ----------- |
| `additionalPrompts`      | string[]            | Extra prompt lines passed to the LLM, inserted between the placeholder warning and its reminder. May be empty. See "Additional prompts" below. |
| `batchSize`              | integer (>= 1)      | Number of entries sent to the translator in a single call. |
| `excludePatterns`        | string[]            | Glob patterns of dot-separated keys to skip (read, translate, write, validate). May be empty. See "Exclude patterns" below. |
| `generateI18nFor`        | object[]            | Target locale files. Each object has `file` (path) and `language` (free-form name passed to the LLM prompt). |
| `hashCacheLocation`      | string *(optional)* | Path where the hash cache lives. Defaults to `.hash-cache.json` in the reference file's directory. |
| `interpolationPatterns`  | string[]            | Regexes matching substrings to keep verbatim during translation (extrapolated to `[0]`, `[1]`, ... before the LLM call and restored afterwards). May be empty. |
| `parallelBatches`        | integer (>= 1)      | Maximum number of batches translated concurrently. |
| `referenceI18n`          | object              | Source locale: `{ "file": "...", "language": "..." }`. |
| `translator`             | enum                | `"claude-code"` or `"gemini-api"`. |

All paths (in `referenceI18n.file`, each `generateI18nFor[].file`, and
`hashCacheLocation`) resolve relative to the directory of the config file.
Absolute paths pass through unchanged.

`language` is the human-readable name written into the LLM prompt — there is
no fixed list. Use whatever phrasing the LLM should see, e.g. `"English"`,
`"Brazilian Portuguese"`, `"Simplified Chinese"`.

All fields except `hashCacheLocation` are required. The skeleton emitted by
`j18n init` includes sensible defaults for `batchSize` and `parallelBatches`.

### Exclude patterns

Patterns operate on dot-separated key paths in the source JSON. Within a
segment, `*` matches any run of characters (no dots) and `?` matches a single
character. Across segments, `**` matches zero or more components.

| Pattern        | Matches                                  | Doesn't match |
| -------------- | ---------------------------------------- | ------------- |
| `sample`       | `sample`                                 | `sampler`, `sample.foo` |
| `sample.**`    | `sample`, `sample.foo`, `sample.foo.bar` | `other.sample` |
| `*.debug`      | `auth.debug`, `pay.debug`                | `debug`, `auth.flow.debug` |
| `**.todo`      | `todo`, `auth.todo`, `auth.x.todo`       | `todoist` |

### Interpolation patterns

Each regex is applied left-to-right, in order, against every value before
translation. Any match is replaced with `[0]`, `[1]`, ... so the LLM sees a
neutral placeholder. After translation the original substrings are spliced
back in. Common examples:

| Style                | Regex                       |
| -------------------- | --------------------------- |
| `{{name}}`           | `\\{\\{(.+?)\\}\\}`         |
| `{0}`, `{count}`     | `\\{[^{}]+\\}`              |
| `%name%`             | `%\\w+%`                    |

Leave the array empty if you don't use interpolations.

### Additional prompts

Lines from `additionalPrompts` are appended to the LLM prompt, in order, after
the generic placeholder warning and before the reminder line. The full prompt
order is:

1. `Translate the values in the following JSON array, from <FROM> to <TO>.`
2. `DO NOT remove or modify HTML tags.`
3. `DO NOT remove, skip or modify placeholders, like [1], [2], [3], etc.`
4. *(your `additionalPrompts` lines, in order)*
5. `Once again, DO NOT remove placeholders like '[1]', '[2]', '[3]', '[4]', etc.`
6. *(translator-specific output-format instructions and the JSON array)*

Use this to give the model domain context, glossary rules, etc.

### Output indentation

The writer auto-detects the indent of the existing target file when syncing,
or otherwise the indent of the reference file. Falls back to a single tab if
neither file has indented lines. The hash cache is always tab-indented.

### Hash cache location and structure

The hash cache tracks which reference values changed between runs. By default
it lives at `<dirname(referenceI18n.file)>/.hash-cache.json`. Override the
location with `hashCacheLocation`:

```json
{
    "referenceI18n": { "file": "locales/en.json", "language": "English" },
    "hashCacheLocation": "build/.j18n-cache.json"
}
```

Useful if you want the cache out of your locales directory or under a
different name.

The file is keyed by target — one entry per `generateI18nFor` element, under
the compound id `<resolved-file>@<language>`:

```json
{
    "src/i18n/locale/pt.json@Brazilian Portuguese": {
        "greeting": "10f79085",
        "farewell": "-8a0ced2"
    },
    "src/i18n/locale/es.json@Spanish": {
        "greeting": "10f79085",
        "farewell": "-8a0ced2"
    }
}
```

Each target's entry is updated and persisted as soon as that target finishes
syncing. If sync fails partway through (say, target #19 of 21), targets 1–18
keep their up-to-date entries — they won't be re-translated on the next run.

#### Migrating an old (flat) cache

`j18n migrate-hash-cache <CONFIG> <HASH_CACHE>` rewrites a flat
`{ "key": "hash", ... }` file into the per-target shape, copying the same
hashing under each target id from the config. Use this once, after upgrading,
if you have existing flat cache files. The subcommand is temporary and will
be removed in a future release.

## Backends

### `claude-code`

Spawns the local `claude` CLI (`cmd /C claude --model=opus -p` on Windows,
`claude --model=opus -p` elsewhere) and pipes the prompt through stdin. Make
sure the `claude` executable is on `PATH`.

### `gemini-api`

Calls the Gemini `generateContent` HTTP endpoint. Requires the `GEMINI_API_KEY`
environment variable; fails fast at startup if it is missing.

## Building

```
cargo build --release -p j18n-cli
```

The binary is written to `target/release/j18n` (`j18n.exe` on Windows).

## Logging

Uses `tracing` with `tracing-subscriber`. Override the level via the
`RUST_LOG` env var, e.g. `RUST_LOG=debug j18n sync ...`. Logs are written to
stderr so stdout is free for piping.

## Testing

```
cargo test --workspace
```

Tests never spawn `claude` or call the Gemini API — both backends are mocked
through their executor / transport traits.
