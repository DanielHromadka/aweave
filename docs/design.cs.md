# aWeave — verzování pro paralelní práci agentů

> **Stav (listopad 2026):** M0 je hotové. Oproti původnímu plánu běží bez daemona: koordinace jde přes
> `.aweave/state.db` (SQLite ve WAL módu), workspaces jsou git worktree. Anglická verze: [design.md](design.md).

> Version control navržený pro situaci, kdy na jednom repu
> současně pracuje 5–50 AI agentů (a pár lidí), ne jeden člověk s jednou větví.

## 1. Proč git nestačí

| Problém v gitu | Dopad při multi-agent práci |
|---|---|
| Jeden working copy + index na checkout (`index.lock`) | Agenti si přepisují soubory, kolize zámků; workaround `git worktree` je těžký (plná kopie, build cache) |
| Konflikty se zjistí až při merge | Dva agenti hodinu upravují stejnou funkci a nikdo o tom neví |
| Merge po řádcích textu | Falešné konflikty (importy, přeformátování), přehlédnuté sémantické kolize |
| Commit = ruční snapshot | Agent spadne → práce mezi commity je pryč; žádný „undo“ celé operace |
| Metadata = autor + zpráva | Chybí *proč*: úkol, model, prompt, nástroje, výsledky testů |
| Push/pull, polling | Agenti nevidí, co ostatní dělají, v reálném čase |

## 2. Klíčové koncepty

### 2.1 Workspace = levný copy-on-write overlay
- Jedno sdílené úložiště objektů, každý agent dostane **workspace** (overlay nad
  základní revizí) za milisekundy — APFS clone / reflink / FUSE.
- Žádný index, žádný staging. Každý zápis do souboru se automaticky zaznamená
  (jako `jj` — working copy *je* změna).

### 2.2 Intent (záměr) a lease
- Agent před prací deklaruje: `aweave intent "fix auth timeout" --scope src/auth/** --symbols AuthService::refresh`.
- Daemon okamžitě hlásí překryv s jinými aktivními intenty → orchestrátor
  může úkol přeplánovat **dřív**, než vznikne konflikt.
- Lease jsou advisory (neblokují), ale mají TTL a jsou vidět všem.

### 2.3 Sémantické diffy a merge
- Diff/merge na úrovni AST (tree-sitter): změna = „upravena funkce X“,
  „přidán import Y“, ne „řádky 40–58“.
- Nezávislé změny ve stejném souboru se mergují automaticky; konflikt jen
  když se dotknou stejného symbolu.
- Fallback na textový 3-way merge pro nepodporované jazyky.

### 2.4 Konflikty jako data
- Konflikt neblokuje — uloží se jako first-class objekt v historii
  (jako `jj` / Pijul) a dá se přidělit k vyřešení jinému agentovi.

### 2.5 Provenance
Každá změna nese strukturovaná metadata (podepsaná):
```
agent: claude-opus-5-5#a17   task: JIRA-123   intent: i_8f2…
tools: [edit×14, bash×3]     tests: 212 passed / 0 failed
parent_session: s_91c…       human_approver: —
```
→ audit (TISAX/ISO 27001), dohledání, který agent co rozbil, revert všeho
od jednoho agenta/úkolu jedním příkazem.

### 2.6 Operation log + undo
Každá operace (snapshot, merge, rebase, intent) je v op-logu. `aweave undo`
vrátí celé repo do stavu před libovolnou operací — bezpečná síť pro agenty.

### 2.7 Integrační fronta místo „merge do mainu“
- Agent změnu *navrhne* → fronta ji automaticky rebasne na aktuální trunk,
  spustí verifikaci (build/testy) a zařadí. Neprošlé změny se vrátí agentovi
  s výstupem.
- Trunk je vždy zelený.

### 2.8 Live sync místo push/pull
- Lokální daemon `aweave (SQLite)` + event stream (subscribe): „agent B změnil
  `User::email`, na kterém závisí tvůj intent“.
- Nativní **MCP server** — agent používá `intent`, `snapshot`, `propose`,
  `watch` jako nástroje, ne parsuje CLI výstup.

## 3. Architektura

```
            ┌──────────── agenti (MCP / CLI) ────────────┐
            │  ws-A        ws-B        ws-C   (CoW overlay)│
            └─────┬───────────┬───────────┬───────────────┘
                  ▼           ▼           ▼
             ┌─────────── aweave (SQLite) (daemon) ───────────┐
             │ intent registry │ event bus │ op log   │
             │ semantic merge  │ integrační fronta     │
             └───────────────┬──────────────────────--┘
                             ▼
             content-addressed store (BLAKE3, chunked)
                             │
                    git bridge (import/export)  →  GitHub / Azure DevOps
```

- **Jazyk:** Rust (výkon, FUSE, tree-sitter bindings, sdílení s `autocut`).
- **Store:** vlastní CAS, ale **obousměrný most do gitu** je nutnost —
  bez něj nikdo nezačne (CI, PR, hosting zůstanou na gitu).

## 4. Prior art (co převzít / čím se lišit)

- **Jujutsu (jj)** — working copy jako commit, op log, first-class konflikty, git backend. Nejbližší; zvážit postavit *nad* jj-lib místo od nuly.
- **Pijul** — teorie patchů, komutující změny.
- **Sapling (Meta)** — stacky, škálování monorepa.
- **GitButler** — virtuální větve v jednom working copy.

Rozdíl aWeave: **koordinace v reálném čase** (intenty, eventy), **sémantický
merge**, **provenance agentů** a **MCP-first** rozhraní. To nikdo z výše
uvedených jako primární cíl nemá.

## 5. MVP (fáze)

1. **M0 – wrapper nad gitem:** daemon + CoW workspaces (git worktree + reflink),
   intent registry, překryvové varování, MCP server. Žádný vlastní store.
   → validace, že intenty reálně snižují konflikty.
2. **M1 – sémantický merge** (tree-sitter: TS, Rust, C#, Python).
3. **M2 – integrační fronta + provenance** (podepsaná metadata v git notes / trailerech).
4. **M3 – vlastní store + op log** (nebo jj-lib), git bridge.

## 6. Otevřené otázky
- Postavit na jj-lib, nebo vlastní jádro?
- Granularita intentů: soubory vs. symboly (symboly vyžadují index/LSP).
- Lokální daemon vs. sdílený server pro agenty na více strojích.
- Bezpečnost: agent nesmí přepsat cizí workspace ani op log → identity + podpisy.
