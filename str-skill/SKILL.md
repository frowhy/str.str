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
2. **NEVER hand-write, hand-edit, `sed`, `perl`, `jq`, or patch a `._meta` file.** One narrow exception exists and is documented below.
3. **NEVER create, rename, move, or delete a UUID directory by hand.** Node/branch directory names are UUIDv7 values owned by the CLI (`str node add`, `str branch add`). A directory named by hand is a format violation (`E_ID_NOT_UUID`, `E_ID_MISMATCH`).
4. **NEVER report success without validating.** After any change inside a bundle, MUST run `str sync <dir>` then `str validate <dir> --strict`, and MUST reach `0 errors, 0 warnings` first.
5. **NEVER invent `._meta` fields.** The key set is closed; unknown keys outside `[ext]` raise `E_SCHEMA_FIELD`. `[ext]` keys MUST be namespaced `vendor.xxx`. When a field's meaning is unclear, ask instead of guessing.
6. **NEVER bypass `str` because it looks unavailable.** Obtain it first (Step 0). Falling back to hand-editing is a rule violation, not a workaround.
7. **MUST read progressively.** Start at `ROOT/._meta`, then drill down. NEVER recursively dump a whole bundle's payloads into context.
8. **NEVER delete a branch with raw `rm`.** Use `str branch rm <dir> <uuid> --force`, which also repairs the parent's `entries[]`.

### Sole exception: fields the CLI cannot set

`v0.1.0` has no command that writes `entries[].title`, `entries[].summary`, `entries[].note`, `tags`, or `authors[]`. Only those fields may be edited directly in `._meta`, and then all three of the following are mandatory:

1. run `str fmt <dir>` (canonical key/table order, comments preserved);
2. bump `revision` by 1 and refresh `updated_at` **in the same edit**, as a TOML native offset date-time with a timezone offset (for example `2026-09-14T10:03:11+08:00`, never a quoted string). Spec §7.2 requires this on every write; note that `str sync` bumps them itself only when it actually changes something;
3. run `str validate <dir> --strict`.

Never touch *structure* this way — `entries[].path`, `entries[].role`, `id`, `kind`, and `refs` belong to the CLI.

## Step 0 — obtain the `str` CLI

Resolve the CLI (building it if a source checkout is nearby) before touching any bundle:

```sh
STR="$(sh <path-to-this-skill>/scripts/ensure-str.sh)" || exit 1
"$STR" --version        # must print: str 0.1.0
```

`ensure-str.sh` resolves the CLI in this order and stops at the first hit:

1. `$STR_BIN` — an explicit path you supply.
2. `str` on `PATH` — accepted only when `--version` prints `str x.y.z`.
3. `$STR_REPO` — a source checkout; builds it with `cargo build --release` when needed.
4. walk up from the script's own directory, then from the CWD, looking for `str-cli/target/release/str` (building from source when it finds `str-cli/Cargo.toml`).
5. **download the prebuilt binary for this platform from GitHub Releases** — detects OS/arch, fetches `str-<tag>-<target>.tar.gz` (`.zip` on Windows), **verifies it against that release's `SHA256SUMS.txt` and refuses to use it when the checksum is absent or does not match**, then caches it at `${XDG_CACHE_HOME:-$HOME/.cache}/str-skill/<tag>/<target>/str`. Later calls reuse the cache without downloading again.
6. otherwise print installation instructions and exit non-zero.

Knobs: `STR_VERSION` (tag, default `latest`), `STR_RELEASE_REPO` (`owner/repo`, for forks), `STR_DOWNLOAD_BASE` (mirror, useful when GitHub is unreachable), `STR_CACHE_DIR`, `STR_NO_DOWNLOAD=1` (offline — local lookup only).

It never falls back to editing files. If it cannot obtain the CLI it fails, and that failure is the correct outcome: report it instead of hand-editing `._meta`.

## Read path (progressive disclosure)

| Intent | Command |
| --- | --- |
| Orient in a bundle | `str tree <dir> --show-refs` |
| List one branch's manifest | `str ls <dir> [--uuid <uuid>]` — the `path` column holds child UUIDs |
| Inspect one branch as JSON | `str show <dir> <uuid>` (`--full` appends payload bodies) |
| Feed a branch to the model | `str context <dir> <uuid> --depth 2 --budget 8000` |
| Machine-readable normal form | `str norm <dir> [uuid]` |
| Whole-tree snapshot | `str export <dir> --format json [--depth n]` |
| Current health | `str validate <dir>` (`--json` for machines) |

Always prefer these over `find`, `ls -R`, `cat`, or `grep` on a bundle.

## Write path

1. Grow the structure with the CLI: `str init`, `str node add`, `str branch add`, `str ref add`.
2. Add or edit **payload and asset files** — anything that is not `._meta` — with the normal file tools. That is allowed and expected.
3. `str sync <dir>` — registers new and removed files and refreshes `size`/`sha256` across all `entries[]`, recursively.
4. `str validate <dir> --strict` — MUST be clean.
5. `str fmt <dir> --check` — MUST report that every `._meta` is already canonical.

Skipping step 3 is the most common failure: the file exists on disk but is unregistered, which is `E_MANIFEST_MISSING` under `manifest = "strict"`.

## Command index

Flag-level detail: `references/cli-reference.md`.

| Command | Purpose |
| --- | --- |
| `str init <dir>` | Create a bundle (ROOT `._meta` + `._schema/`) |
| `str validate <dir>` | Full validation, all error codes |
| `str tree <dir>` | Render the branch tree (`--show-refs` adds link lines) |
| `str ls <dir>` | List a branch's manifest |
| `str show <dir> <uuid>` | Print a branch `._meta` as normalised JSON |
| `str node add <dir>` | Add an independent node (depth 1) |
| `str branch add <dir> <anchor>` | Add a related branch at any depth |
| `str branch rm <dir> <uuid> --force` | Delete a branch and everything below it |
| `str ref add <dir> <uuid> --target <uuid>` | Add a cross-branch link |
| `str ref rm <dir> <uuid> --ref <ref-id>` | Remove a cross-branch link |
| `str sync <dir>` | Reconcile `entries[]` with disk |
| `str fmt <dir>` | Rewrite `._meta` in canonical order, keeping comments |
| `str norm <dir> [uuid]` | Normalised JSON on stdout |
| `str context <dir> <uuid>` | AI context fragment (Markdown) |
| `str export <dir>` | Read-only export to a single document |
| `str reveal <dir>` | macOS bundle bit / unhide `._meta` |
| `str codes` | List all error codes |

## Resources

- `references/cli-reference.md` — exact command surface, flags, exit codes, output shapes, and the **spec-versus-implementation drift list**. Read it before using any flag remembered from the spec: several are not implemented.
- `references/spec-digest.md` — distilled format rules: depth semantics, naming, `._meta` field tables, policies, error codes.
- `references/workflows.md` — copy-paste recipes, each verified end to end against the real CLI.
- `scripts/ensure-str.sh` — resolve or build the CLI.

## Anti-patterns seen in practice

| Anti-pattern | Why it fails | Do instead |
| --- | --- | --- |
| `cat bundle/._meta` and claim to "understand the tree" | `._meta` is one projection; whole branches stay invisible | `str tree`, `str context` |
| `sed -i 's/old/new/' ._meta` | Breaks key order, `revision`, `updated_at`, comments, and digests | CLI first; see the sole exception |
| Writing `notes.md` and stopping | `E_MANIFEST_MISSING` under `manifest = "strict"` | `str sync`, then `str validate --strict` |
| `mkdir 客户档案` as a branch | `E_ID_NOT_UUID` | `str node add` / `str branch add` |
| `rm -rf <uuid-dir>` | Parent `entries[]` keeps a ghost entry (`E_MANIFEST_GHOST`) | `str branch rm ... --force` |
| `str validate` before `str sync` | Reports the drift just created, not a real defect | `sync` first |
| Trusting `STR-FORMAT-PROMPT.md` §9 flags verbatim | Several documented flags do not exist | `references/cli-reference.md` |
