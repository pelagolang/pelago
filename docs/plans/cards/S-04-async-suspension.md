Id: S-04
Title: async: suspension in the middle of a body (state machines)
Column: Shelf
Order: after S-02 and A-04
Depends on: S-02 (real threads or an executor), A-04 (the optimiser shrinks state machines), A-07 stage 0 (done)
Size: XL
Revisit when: a program needs thousands of concurrent waits, or the backend can run an executor.

# What exists

`async fn`, `await`, `future<T>` and `block_on` work at stage 0 (A-07): a call runs the body to the end. Nothing can be waiting in the middle.

# What this card adds

The plan in `A-07-async.md` section "What the work would be": stackless coroutines by state machine, generated in the lowering; the checker rule that nothing that is a
view or `inout` may live across an `await`; a library executor (`block_on` and a small single-threaded one) so that "no runtime" stays true; `select` over futures;
cancel by drop with the ledger (A-03) dropping every live local at an `await`.

Open questions then: cancellation semantics, async in interfaces (`any` over async methods), `async` closures and blocks.
