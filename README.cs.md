<div align="center">

<img src="website/img/icon.svg" alt="Ikona aWeave" width="104" height="104">

# aWeave

**Hodně agentů. Jedno repo. Žádné srážky.**

Koordinace verzování pro týmy AI agentů, kteří pracují na jednom git repozitáři současně.

[Web](https://aweave.pages.dev/) · [Návrh](docs/design.cs.md) · [English](README.md)

</div>

## Proč

Git počítá s člověkem, který dělá jednu věc a občas mergne. Deset agentů na jednom repu to rozbije: přepisují si
soubory, o konfliktech se dozví až při merge, nikdo neví, kdo na čem dělá, trunk se rozbíjí a z commitu nepoznáš,
který agent a úkol za změnou stojí.

aWeave přidává chybějící koordinační vrstvu. Všechno dál končí v gitu, takže GitHub, CI i editor fungují beze změny.

## Jak to funguje

| Pojem | Co dělá |
|---|---|
| **Workspace** | Git worktree v `.aweave/ws/<jméno>` na větvi `aweave/<jméno>`, jedním příkazem. Pozná se z aktuálního adresáře. |
| **Intent (záměr)** | „Chystám se měnit tyhle cesty“ (soubory, adresáře, globy) s platností. Po merge se uvolní sám. |
| **Radar překryvů** | Porovná každou dvojici: oba změnili stejný soubor, jeden změnil, co druhý zamýšlí, nebo se kryjí záměry. `aweave check` vrací kód 2. |
| **Snapshot** | Commit všeho ve workspace s trailery `Aweave-Agent`, `Aweave-Workspace`, `Aweave-Task`, `Aweave-Intents`. |
| **Integrační fronta** | `propose` rebasne workspace na trunk v oddělené worktree, spustí verify a trunk posune, jen když projde. Vždy jeden návrh naráz, konflikt ani pád testů trunk nezmění. |
| **Event log** | Všechny akce v pořadí; `aweave events --follow` nebo MCP nástroj `aweave_events`. |

```console
$ aweave init --verify "cargo test"
$ aweave --agent claude-1 ws new auth-timeout
$ aweave --ws auth-timeout intent add "Oprava timeoutu session" --scope src/auth/session.rs
$ aweave snapshot -m "Retry session refresh" --task SHOP-142
$ aweave propose
```

Stav je jeden SQLite soubor (`.aweave/state.db`, WAL), agenti v různých procesech se tak koordinují bez daemona.

## Instalace

```bash
curl -fsSL https://aweave.pages.dev/install.sh | sh          # ověří SHA-256
cargo install --git https://github.com/DanielHromadka/aweave  # ze zdrojáků, Rust 1.89+
```

## MCP pro Claude Code, Codex, Cursor

```bash
cd "$(aweave --agent backend-1 ws new backend-1)"
claude mcp add aweave -- aweave mcp --agent backend-1
```

Nástroje: `aweave_status`, `aweave_workspace_create`, `aweave_intent_declare`, `aweave_intent_release`,
`aweave_snapshot`, `aweave_sync`, `aweave_propose`, `aweave_events`.

## Plán

M0 (tohle) je koordinace nad gitem. Dál: **M1** sémantický merge po funkcích (tree-sitter), **M2** podepsaná
provenance a revert podle agenta či úkolu, **M3** vlastní úložiště s op-logem, CoW workspaces a `undo`. Podrobně v
[docs/design.cs.md](docs/design.cs.md).

## Vývoj

```bash
cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check
```

## Licence

[GNU GPL v3.0 nebo novější](LICENSE).
