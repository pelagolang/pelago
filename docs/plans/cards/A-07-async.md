Id: A-07
Title: `async` / `await`
Column: Done at stage 0 (the owner asked for everything to be completed): eager, run-to-completion futures. True suspension moved to S-04.
Order: 7
Depends on: self-hosting compiler (the decision G5 says to revisit then), S-02 (real threads), A-04 (an optimiser that can shrink state machines)
Size: XL
Revisit when: the compiler compiles itself, or a concrete program needs 10,000 or more concurrent waits that threads cannot serve.

# Why it is on the shelf and not simply "not done"

This one is deferred by a **decision** (`functions.md` section 17, G5, `concurrency.md`), for reasons that still hold:

- A function that can pause must keep its locals somewhere. Under value semantics a borrowed `inout` or a view cannot be held across a pause (the thing it borrows could change). Rust solved this with a large and difficult
  type-system feature (pinning, self-referential futures). We have not paid for that complexity.
- Threads and channels, `shared`, `lock` and `select` already give concurrency. In stage 0 they are cooperative; with S-02 they are real.
- "No runtime" (a project decision) is in tension with an async scheduler. An executor *is* a runtime.

The words `async` and `await` are reserved and parse. Using them today is an error "not supported yet".

# What the work would be (so nobody has to rediscover it)

If the owner reopens it, the least costly design that fits this language:

1. **Stackless coroutines by state machine**, generated in the lowering: an `async fn` becomes a struct holding its locals and a state number; `await` is a state boundary. The checker rule: *nothing that is a view or `inout` may live across an `await`* (this is exactly the existing "view escape" analysis, applied to the await point). Everything else is an ordinary owned local and is saved in the state struct.
2. **No built-in executor.** `async fn` returns `future<T>`; the library offers `blockOn(f)` and a small single-threaded executor written in the language. The program chooses its executor. This keeps "no runtime" true: the runtime is a library, like `chan` today.
3. **`await` inside loops and `?`** must work with the ledger (A-03) and drop rules: every live owned local at an `await` is part of the saved state and is dropped if the future is dropped before it finishes (cancel by drop).
4. **Interaction with `select`:** `select` over futures instead of channels; the cooperative `select` already written is the scalar form of this.
5. **Cost model:** a future's size is known at compile time (sum of the largest live set), so it can live on the stack or in a `box`. No hidden allocation, in line with rule TC4.

Open questions then: cancellation semantics, async in traits (`any` over async methods), and whether `async` closures exist.

# Not now because

Nothing in the compiler, the library, or the self-hosting goal needs it, and it is the largest single feature on the list. It belongs after the compiler can build itself.
