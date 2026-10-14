Id: B-01
Title: Choose the language name
Column: Done (Pelago chosen)
Order: 1 (track B)
Depends on: nothing
Size: S
Touches: `NAMES.md`, then B-02 and B-04

# 1. What the Wikipedia research says about names

Pages read: Domain-specific language, Markup language, Esoteric programming language, BASIC, List of programming languages by type, Timeline of programming languages,
Generational list of programming languages, List of programming languages (the P section and the whole list).

**Patterns in how successful languages are named**

1. **Old languages (1950s to 1970s) are acronyms or descriptions:** FORTRAN, COBOL, BASIC, ALGOL, SNOBOL, MUMPS. They are hard to remember and have no personality; few new languages do this.
2. **From the 1980s the names get a face.** Evocative single words appear: Oberon, Perl (gem), Eiffel (place), Mercury (god and planet), then in the 1990s Python, Lua ("moon"), Java (island), Ruby, Euphoria. Mythology, places, gems and animals dominate.
3. **Recent systems languages use plain, short, odd words:** Rust, Go, Zig, Nim, Swift, Pony, Julia, Crystal, Chapel, Gleam, Elixir. Mostly 2 to 6 letters, one or two syllables, and an ordinary English word or a coined sound.
4. **Family names pay off** (BASIC): when many dialects keep the core word and add a qualifier (Visual Basic, FreeBASIC, QBasic), the name carries recognition. The cost: a very generic core word is hard to search. A name needs to be distinctive enough that "name + lang" finds the project.
5. **Do not signal a narrow domain.** The DSL page shows the suffixes that mean "small special-purpose language": Markup, Query, Configuration, Description, Shading, Script. A general systems language must avoid them (so no "-Script", no "-ML", no "-QL").
6. **Esoteric languages show themes** (cooking, literature, art, mythology, internet jokes) and one warning: Piet was meant to be Mondrian, but the name was taken by software. Name clashes with existing software are a real, recurring problem.
7. **Ocean and sea names are almost unused.** In the whole of the lists the only water or geography words are islands and places (Java, Ceylon, Pico) and a city (Darwin). **No language is named after an ocean.** That is an open lane, which matches your idea.
8. **File extensions:** the Markup page shows how many short extensions are already spoken for by document formats (`.md`, `.tex`, `.rst`, `.xml`, `.html`, `.toml`, `.yaml`). Check each candidate extension against those and against other languages (`.pl` is Perl and Prolog, `.ml` is OCaml, `.tl` is Teal, `.pc` is pkg-config).

# 2. Candidate review (web checks, same method as `NAMES.md`)

Legend: ✅ nothing found, ⚠️ something to watch, ❌ clash.

| Name | Meaning | Check | Notes |
|---|---|---|---|
| **Pacific** | The largest ocean | ✅ No programming language found on GitHub or in searches. (I could not find the "Pacific" language you remember. Please send a link or the era if you can recall one.) | ⚠️ Very hard to search: "Pacific language" returns the human languages of the Pacific. A common word with many products. Extension candidates are crowded: `.pc` (pkg-config), `.pac` (proxy config). |
| **Atlantic** | The second largest ocean | ✅ No programming language found. | ⚠️ "Atlantic languages" is a real linguistic family (Niger-Congo), so searches are polluted. `.atl` is the Eclipse ATL transformation language. A magazine and a record label also own the name. |
| **Thalassa** | Greek for "the sea" (and the primordial sea goddess) | ✅ No language found. | Distinct and searchable. Three syllables, 8 letters. Compiler command could be `thalassa` with the short alias `thal`. Extensions to consider: `.tha`, `.thl`. (`.tl` is Teal.) ⭐ |
| **Panthalassa** | "All sea": the single world ocean around the supercontinent Pangaea | ✅ No language found. | ⚠️ A US company named Panthalassa (ocean wave energy for compute) raised a large round in 2026, so the name is in the news. Long (11 letters). Fits "vast expanse" best of all. Could be the long name with the short form `Panta`. |
| **Pontus** | Greek primordial god of the sea; also an ancient region | ✅ No language found. | Short, strong, easy to say. Two syllables. Commands: `pontus`; ext `.pn` or `.pts`. ⭐ |
| **Oceanus / Okeanos** | The Titan of the encircling ocean | ✅ No language. | ⚠️ A Tencent Cloud stream-processing product (OCEANUS) and a CMS use it. Long. |
| **Tethys** | Titaness of the sea; also a moon | ❌ A toy functional language already exists (ThePuzzlemaker/tethys) and a mission-scripting language (TethysL) | Skip. |
| **Nereid** | A sea nymph; a moon of Neptune | ✅ none found | Cute, uncommon; harder to spell. |
| **Gyre** | A vast rotating ocean current (the Pacific and Atlantic each have gyres) | ✅ No language. | ⚠️ "Gyre" is the name of a compiler *intermediate representation* project (GyreD, inactive, 2 stars). Very close to our own territory; could confuse. Short and elegant. |
| **Fathom** | Unit of sea depth; "to understand" | ❌ A binary-format language (yeslogic/fathom) plus several products and a Rust crate | Skip. |
| **Atoll** | A ring-shaped coral island | ✅ none found | Nice picture (a ring of small parts forming a whole), short. Search results thin, which is good. |
| **Pelago** | Short for archipelago | ✅ none | Not a real English word, so it reads as invented. |
| **Offing** | The distant part of the sea in view ("in the offing" = about to happen) | ✅ none | Quirky; but a common noun, hard to search. |
| **Kai** | Hawaiian "sea" | ❌ A self-hosted systems language named Kai already exists on GitHub | Skip. |
| **Halcyon** | Calm-sea bird and "peaceful" | ❌ Two language projects (HalcyonScript and a WebAssembly language) | Skip. |
| **Moana** | Polynesian "ocean" | not searched | Strong brand association (a major film). Skip. |

# 3. Recommendation

1. **Thalassa** (first): ocean-themed, no clash, distinct, searchable ("thalassa lang" finds nothing else), pronounceable, has a long mythological meaning, and the nickname "Thal" gives a short command and extension.
2. **Pontus** (second): the shortest and punchiest on the list; same mythological root.
3. **Pacific** (if you want the exact word): it is the most evocative, but expect search pollution; it needs a qualifier in practice ("Pacific lang"). `Pacific` + file extension `.pac` is the problem.

A middle path used by other languages: take the long evocative name and a short tool name (the language "Panthalassa", the tool `thal`, the extension `.thl`).

**A note on the project rule:** `CLAUDE.md` says "avoid nature-themed names". This round follows your new request (oceans), so I am treating the ocean theme as a deliberate exception. If you pick an ocean name, update that line of `CLAUDE.md` in the same commit as the decision.

# 4. Questions for the owner

1. Which name, or which two to take to a final check (domain names, crates.io, npm, GitHub organisation, PyPI)?
2. One tool named after the language (like `cargo` or `zig`) with the compiler inside, or the language name plus `c` (`thalassac`)? **Recommendation: one tool**, as in `NAMES.md` round 6.
3. If you pick Pacific or Atlantic, accept the search problem and plan for it (an official "lang" suffix in docs and the site)?
4. Is the "vast expanse" idea about size (a language that scales), calm, or reach? Knowing which helps choose between Thalassa (the sea itself), Panthalassa (all of it), and Pontus (the deep).


# 5. Head to head: Pelago and Thalassa (research after round 1)

Checked October 2026 with direct registry lookups and web searches.

| | **Pelago** | **Thalassa** |
|---|---|---|
| Meaning | Greek *pelagos* "the open sea"; the root of *archipelago* ("chief sea", then "sea of islands") | Greek "the sea"; the sea goddess |
| Letters, syllables | 6, two (peh-LAY-go; some say peh-LAH-go) | 8, three (thuh-LASS-uh) |
| crates.io | free | **taken** (a TLS wire-format library, 2026) |
| npm | free | **taken** (an old service registry, 12 years) |
| PyPI | free | **taken** (an ocean-modelling package) |
| Language clash | none found | none found |
| Other products | Singapore Airlines' travel marketplace (pelago.com), a Santa Barbara project-management firm (Intervals), a defunct Seattle app (Whrrl), an indie game (Puzzle Pelago). None is a programming tool. | None found in software; French TV and spas use the word. |
| Domains | `pelago.com` / `.co` taken; `.dev`, `.org`, `.io` resolve (parked or used); `pelagolang.org` and `pelago-lang.org` do not resolve (probably free, confirm at a registrar) | not checked |
| Search | "pelago" alone returns the airline app; "pelago lang" should be clean | "thalassa" returns spas, TV and the packages above |
| Typing the tool | `pelago build`, short and easy | `thalassa build` is long; needs the alias `thal` |
| Extension candidates | `.pel` (no known clash), `.pg` (clashes with PostgreSQL dumps), avoid `.pl` | `.tha`, `.thl` |

## The ecosystems that grow out of each name

**Pelago** gives a vocabulary that matches how the language is built (it has `pkg` and `mod`, so many small units):
- *Archipelago* is a real, natural word for **a workspace of many packages** ("an archipelago of islands in one sea"). The package manager can call the workspace file `archipelago` or keep a plain name and use the word in docs.
- A **package** is an island; the **standard library** is the open sea they all stand in (`core`). "Island" is for talk and docs; the keyword stays `pkg`.
- The **registry** could be "the Strait" or simply `pelago.dev/pkg`; the **community** "islanders"; releases named by a chain: **Aegean** (1.x), ... (a naming list of real seas is endless).
- Tool: **one tool**, `pelago` with `build`, `run`, `test`, `fmt`, `check`, `highlight`. The compiler crate can stay `pelagoc` inside it.
- Mascot ideas that avoid clichés: a pelagic bird (an albatross), a chain of islands drawn as dots, an open horizon line. Colours: sea blue plus one warm accent.
- Slogans in its own voice: "Open water, solid ground." "Small islands, one sea." "Stay afloat."

**Thalassa** gives a grander but heavier vocabulary: the tool `thalassa` (alias `thal`), libraries as "currents" or "tides", releases named after sea gods. Beautiful, but the spelling (two `s`, double `l`?) and length cost every user a little each day, and it starts life with three registry names already taken, so every package would be `thalassa-lang` or similar.

## Reading

- Pelago is **shorter, easier to type, free on all three registries**, and its meaning (the open sea, the root of archipelago) fits both "vast expanse" and a language built from many small packages.
- Its cost is brand collision **outside software**: one airline-owned travel brand is very visible. That is the same kind of overlap that Go, Swift, Ruby, Python and Java live with (all existing words), and none of the Pelago businesses is a developer tool.
- Thalassa is cleaner of competing travel brands but **loses on length and on registries**.

Recommendation: **Pelago**, with the tool named `pelago`.

## Before committing (a short checklist; none blocks the decision)

1. Register `pelago-lang` (or `pelagolang`) as the GitHub organisation, and reserve `pelago` on crates.io, npm and PyPI (empty placeholder packages).
2. Confirm `pelagolang.org` / `pelago-lang.org` at a registrar.
3. Check trademark databases (USPTO, EUIPO) in the software classes (9, 42) before any public release.
4. Decide the pronunciation we will use in docs (**peh-LAY-go**) and say it once on the website.

# Done means

- The owner has chosen a name; `NAMES.md` has the decision at the top and the rest struck through.
- `CLAUDE.md`, `README.md`, `ROADMAP.md` say the final name; B-02 and B-04 are unblocked.
