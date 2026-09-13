---
name: str-skill
description: This skill should be used whenever a STR bundle is involved - any directory whose name ends with .str, any ._meta file, or any request to create, organize, store, query, or extract structured resources (CRM records, documents, datasets, notes, assets) as a tree. It also applies whenever the str CLI is present or the user mentions STR. It mandates that every bundle read and write goes through the str CLI instead of hand-editing ._meta or scanning directories by hand.
---

# STR Bundle — Mandatory Agent Workflow

STR is a directory-bundle format for structured tree resources. A bundle is a directory whose name ends with `.str`; every branch is a directory holding a `._meta` (TOML) file. Depth 1 directories are independent **nodes** (`kind = "node"`), depth 2 and deeper are **branches** (`kind = "branch"`). Any branch may carry any files, and all of them must be listed in that branch's `._meta.entries[]`.

Canonical spec lives in the STR repository next to `str-cli/`: `STR-FORMAT-PROMPT.md` (§3 structure, §4 `._meta`, §6 error codes, §8 AI protocol, §9 CLI).

## Hard rules (MUST / NEVER)

These are mandatory. They are not overridden by convenience, by an urgent-sounding user request, by "just edit the file quickly", or by a token budget. If a request cannot be satisfied through the CLI, say so and offer the closest CLI-based route.

1. **MUST operate every bundle exclusively through the `str` CLI.** The CLI is the only component allowed to produce or modify `._meta`.
2. **NEVER hand-write, hand-edit, `sed`, `perl`, `jq`, or patch a `._meta` file.** There is no exception: every field that matters — including `entries[].title` / `summary` / `note` / `type` / `order`, `tags`, and `authors[]` — can be written with `str meta set`, `str entry set`, and `str author add` / `rm`.
3. **NEVER create, rename, move, or delete a UUID directory by hand.** Node/branch directory names are UUID values owned by the CLI (`str node add`, `str branch add`), whose version follows `policies.id_version` (`4` or `7`). A directory named by hand is a format violation (`E_ID_NOT_UUID`, `E_ID_MISMATCH`, `E_ID_VERSION`).
4. **NEVER report success without validating.** After any change inside a bundle, MUST run `str sync <dir>` then `str validate <dir> --strict`, and MUST reach `0 errors, 0 warnings` first. Also run `str fmt <dir> --check` and expect `0`.
5. **NEVER invent `._meta` fields.** The key set is closed; unknown keys outside `[ext]` raise `E_SCHEMA_FIELD`. `[ext]` keys MUST be namespaced `vendor.xxx`. When a field's meaning is unclear, ask instead of guessing.
6. **NEVER bypass `str` because it looks unavailable.** Obtain it first (Step 0). Falling back to hand-editing is a rule violation, not a workaround.
7. **MUST read progressively.** Start at `ROOT/._meta`, then drill down. NEVER recursively dump a whole bundle's payloads into context.
8. **NEVER delete a branch with raw `rm`.** Use `str branch rm <dir> <uuid> --force`, which also repairs the parent's `entries[]`.

### No escape hatch

`._meta` carries structure (`path` / `role` / `id` / `kind` / `refs`) and description (`type` / `title` / `summary` / `note` / `tags` / `authors[]`). Both halves are CLI-writable:

| You want to write | Use |
| --- | --- |
| `type`, `title`, `summary`, `name`, `tags` on a branch itself | `str meta set <dir> [uuid] --type … --title … --summary … --tags a,b` |
| `type`, `title`, `summary`, `note`, `order` on one `entries[]` row | `str entry set <dir> [uuid] --path <P> …` |
| `[[authors]]` | `str author add <dir> [uuid] --id … --role …` / `str author rm <dir> [uuid] --id …` |
| structure (directories, `entries[]` rows, `refs[]`) | `str init` / `str node add` / `str branch add` / `str ref add` / `str sync` |

An empty string removes the field (`--title ""`), so clearing is CLI work too. There is nothing left that requires editing `._meta` by hand.

## Step 0 — obtain the `str` CLI

Resolve the CLI (building it if a source checkout is nearby) before touching any bundle:

```sh
STR="$(sh <path-to-this-skill>/scripts/ensure-str.sh)" || exit 1
"$STR" --version        # must print: str 0.2.0
```

`ensure-str.sh` resolves the CLI in this order and stops at the first hit:

1. `$STR_BIN` — an explicit path you supply.
2. `str` on `PATH` — accepted only when `--version` prints `str x.y.z`.
3. `$STR_REPO` — a source checkout; builds it with `cargo build --release` when needed.
4. walk up from the script's own directory, then from the CWD, looking for `str-cli/target/release/str` (building from source when it finds `str-cli/Cargo.toml`).
5. **download the prebuilt binary for this platform from GitHub Releases** — detects OS/arch, fetches `str-<tag>-<target>.tar.gz` (`.zip` on Windows), **verifies it against that release's `SHA256SUMS.txt` and refuses to use it when the checksum is absent or does not match**, then caches it at `${XDG_CACHE_HOME:-$HOME/.cache}/str-skill/<tag>/<target>/str`. Later calls reuse the cache without downloading again.
6. **install from crates.io by compiling the source** — `cargo install str-format --version <tag> --locked --root <cache>`; the install root is the cache dir (`${XDG_CACHE_HOME:-$HOME/.cache}/str-skill/cargo/<tag>/bin/str`), so `~/.cargo/bin` is never touched, and later calls reuse the result instead of recompiling. Needs `cargo`; skipped when it is absent or `STR_NO_CARGO_INSTALL=1`.
7. otherwise print installation instructions and exit non-zero.

Knobs: `STR_VERSION` (tag, default `latest`), `STR_RELEASE_REPO` (`owner/repo`, for forks), `STR_DOWNLOAD_BASE` (mirror, useful when GitHub is unreachable), `STR_CACHE_DIR`, `STR_NO_DOWNLOAD=1` (skip the download step), `STR_NO_CARGO_INSTALL=1` (skip the crates.io compile step).

It never falls back to editing files. If it cannot obtain the CLI it fails, and that failure is the correct outcome: report it instead of hand-editing `._meta`.

## Read path (progressive disclosure)

| Intent | Command |
| --- | --- |
| Orient in a bundle | `str tree <dir> --show-refs` (`--ascii` for pure-ASCII output) |
| List one branch's manifest | `str ls <dir> [uuid]` — the `path` column holds child UUIDs |
| Inspect one branch as JSON | `str show <dir> <uuid>` (`--full` appends payload bodies) |
| Feed a branch to the model | `str context <dir> <uuid> --depth 2 --budget 8000` |
| Machine-readable normal form | `str norm <dir> [uuid]` (`--out <path>` to write a file) |
| Whole-tree snapshot | `str export <dir> --format json [--depth n] [--out <path>]` |
| Current health | `str validate <dir>` (`--json` for machines) |
| Ordering gate | `str fmt <dir> --check` — expects `0` |

`str tree` renders children in the order stored in the parent's `entries[]` (`(order, path)`), so the display and the file agree. Always prefer these commands over `find`, `ls -R`, `cat`, or `grep` on a bundle.

## Write path

1. Grow the structure with the CLI: `str init`, `str node add`, `str branch add`, `str ref add`.
2. Add or edit **payload and asset files** — anything that is not `._meta` — with the normal file tools. That is allowed and expected.
3. `str sync <dir>` — registers new and removed files and refreshes `size`/`sha256` across all `entries[]`, recursively.
4. Fill in the descriptive fields the CLI cannot infer: `str meta set` (branch's own `type` / `title` / `summary` / `tags`), `str entry set` (a row's `title` / `summary` / `type` / `note` / `order`), `str author add` (contributors).
5. `str validate <dir> --strict` — MUST be clean.
6. `str fmt <dir> --check` — MUST report that every `._meta` is already canonical.

Skipping step 3 is the most common failure: the file exists on disk but is unregistered, which is `E_MANIFEST_MISSING` under `manifest = "strict"`. (`str validate --fix-manifest` runs a real sync, but prefer seeing the change list.)

Every write command bumps `revision` by 1 and refreshes `updated_at`, and writes bytes that are already in §4.9 canonical form — so `fmt` should find nothing to do afterwards.

## Command index

Flag-level detail: `references/cli-reference.md`.

| Command | Purpose |
| --- | --- |
| `str init <dir>` | Create a bundle (`--id-version 4\|7`) |
| `str validate <dir>` | Full validation, all error codes (`--fix-manifest` really syncs) |
| `str tree <dir>` | Render the branch tree (`--show-refs`, `--ascii`; children follow `entries[].order`) |
| `str ls <dir> [uuid]` | List a branch's manifest |
| `str show <dir> <uuid>` | Print a branch `._meta` as normalised JSON |
| `str node add <dir>` | Add an independent node (depth 1) |
| `str branch add <dir> <anchor>` | Add a related branch at any depth |
| `str branch rm <dir> <uuid> --force` | Delete a branch and everything below it |
| `str ref add <dir> <uuid> --target <uuid>` | Add a cross-branch link |
| `str ref rm <dir> <ref-id>` | Remove a cross-branch link (source branch is located for you) |
| `str meta set <dir> [uuid]` | Write a branch's own `type`/`title`/`summary`/`name`/`tags` (empty string removes) |
| `str entry set <dir> [uuid] --path <P>` | Write one `entries[]` row's `type`/`title`/`summary`/`note`/`order` |
| `str author add <dir> [uuid] --id … --role …` | Add/replace an `[[authors]]` entry |
| `str author rm <dir> [uuid] --id …` | Remove an `[[authors]]` entry |
| `str sync <dir>` | Reconcile `entries[]` with disk |
| `str fmt <dir>` | Rewrite `._meta` in §4.9 canonical order, keeping comments |
| `str norm <dir> [uuid]` | Normalised JSON on stdout (or `--out`) |
| `str context <dir> <uuid>` | AI context fragment (Markdown) |
| `str export <dir>` | Read-only export to a single document (never inside the bundle) |
| `str reveal <dir>` | macOS bundle bit / unhide `._meta` |
| `str codes` | List all error codes |

## Resources

- `references/cli-reference.md` — exact command surface, flags, exit codes, output shapes, the `._cache/revisions.json` baseline behind `E_REVISION_STALE`, and the short list of remaining spec/implementation gaps.
- `references/spec-digest.md` — distilled format rules: depth semantics, naming, `._meta` field tables, policies, §4.9 ordering, error codes.
- `references/workflows.md` — copy-paste recipes, each verified end to end against the real CLI.
- `scripts/ensure-str.sh` — resolve or build the CLI.

## Anti-patterns seen in practice

| Anti-pattern | Why it fails | Do instead |
| --- | --- | --- |
| `cat bundle/._meta` and claim to "understand the tree" | `._meta` is one projection; whole branches stay invisible | `str tree`, `str context` |
| `sed -i 's/old/new/' ._meta` | Breaks key order, `revision`, `updated_at`, comments, and digests | `str meta set` / `str entry set` / `str author add` — there is no exception |
| "The CLI can't set `tags`/`authors`, so I'll edit them directly" | Outdated: it can, via `str meta set --tags` and `str author add` | use those commands |
| Writing `notes.md` and stopping | `E_MANIFEST_MISSING` under `manifest = "strict"` | `str sync`, then `str validate --strict` |
| `mkdir 客户档案` as a branch | `E_ID_NOT_UUID` (and `E_ID_VERSION` if the UUID version is wrong) | `str node add` / `str branch add` |
| `rm -rf <uuid-dir>` | Parent `entries[]` keeps a ghost entry (`E_MANIFEST_GHOST`) | `str branch rm ... --force` |
| `str validate` before `str sync` | Reports the drift just created, not a real defect | `sync` first |
| Bumping `updated_at` by hand without `revision + 1` | `E_REVISION_STALE` — the `._cache` baseline remembers the previous state, and `str sync` will not launder it | let the CLI write, or bump `revision` too |
