# Plans: how we track work

This folder is the project's task board. It is written for **any LLM or person who picks the work up later**, so
read this file first, then `BOARD.md`, then the card you are about to work on.

## The system (a small Kanban)

There is one **board** (`BOARD.md`) and one **card** per piece of work (`cards/NN-slug.md`). A card is a plan in full:
why, design, steps, tests, and how we know it is done. The board only lists cards by column.

### Columns

| Column | Meaning | Who moves a card out |
|---|---|---|
| **Inbox** | An idea or a note. Not designed yet. | The owner or an LLM, when it is written up as a plan. |
| **Needs decision** | The plan exists but one or more numbered questions wait for the owner. Nothing may be built yet. | The owner answers; record the answers in the card **and** in the matching design doc and `ROADMAP.md`. |
| **Ready** | Fully specified and unblocked. Anyone may start it. Ordered top to bottom: **take the first one**. | Whoever starts it. |
| **Doing** | Someone is working on it now. **At most one card at a time per track** (the WIP limit), so work finishes before more starts. | The worker, when the "Done means" checklist in the card is true. |
| **Review** | Built and green (fmt, clippy, tests). Waiting for the owner to look. | The owner. |
| **Done** | Finished and accepted. Kept as a record; the card says what was built and where. | nobody |
| **Shelf** | Deliberately parked. Every shelved card says *why* and *what event should bring it back* (the "revisit when" line). | The owner, or whoever sees the revisit trigger happen. |

### Tracks

Work runs on two tracks so the owner-only questions (such as the name) never block the compiler.

- **Track A, compiler:** the language and the IR.
- **Track B, tooling and identity:** the name, the file extension, editor support.

### Card header

Every card starts with this block, kept up to date when the card moves:

```
Id: A-02           (A or B for the track, S for shelf-only ideas, D for done records)
Title: box<T>
Column: Ready
Order: 2           (position inside its track; lower goes first)
Depends on: A-01
Size: L            (S under a day, M a few days, L about a week, XL more)
Touches: checker, values pass, lowering, core.lang, docs
```

### Rules for working a card

1. Take the first card in **Ready** on your track. If the next card in order is waiting in **Needs decision**, take the next **Ready** card that does not depend on it, and tell the owner the decision is blocking. Move it to **Doing** (edit the board and the card header in the same commit).
2. Follow the card's steps in order. If the plan turns out wrong, change the card first, then the code. Plans are not sacred; stale plans are.
3. The card's **Done means** list is the exit test. Every item must be true, including docs.
4. Keep `cargo fmt --all --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test` green.
5. When a decision is made, write it into the relevant `docs/design/` file and the `ROADMAP.md` decisions table in the **same commit** (this is the project rule).
6. Finish by moving the card to **Review** and writing three lines at the bottom of the card: what was built, what changed from the plan, what is left over.
7. New ideas found on the way go to **Inbox** as a card stub (header and one paragraph). Do not start them.
8. Shelf cards are not forgotten: at the start of each phase, scan the shelf for revisit triggers that have happened.

### Why plans are this long

The owner is budget-conscious and works with several LLM sessions. A long plan costs tokens once; a vague plan costs them
every time someone has to rediscover the design. Each card therefore carries the reasoning, the rejected options and the
test list.
