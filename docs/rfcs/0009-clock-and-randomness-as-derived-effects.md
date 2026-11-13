# RFC 0009: The clock and randomness as derived effects

- Status: accepted (decided by the project lead on 2026-11-13, with the refinements recorded in Design; implemented in `compiler/unio/src/effects.rs`, `stdlib/std/time.unio`, `stdlib/std/random.unio` and `compiler/unio-runtime/src/externs/clock.rs`, see Verification)
- Author: Unio project, with Claude
- Date: 2026-11-12, accepted 2026-11-13
- Affects: semantics (docs/semantics.md 8.2, 8.3) / standard library (`std.time`, `std.random`, requirement S-1) / tools (`unio symbol`)
- Russian localization: [0009-clock-and-randomness-as-derived-effects.md](../ru/rfcs/0009-clock-and-randomness-as-derived-effects.md)

## Summary

Two derived effects join `files.read`, `files.write`, `network`, `process`, `env` and `allocates`: `clock`, reading the current time, and `random`, obtaining randomness from outside the program. Like `allocates`, they need no permission of the manifest and are never E0085; `unio symbol` lists them, and they travel through calls, interfaces and function values like every other effect. Cryptographic randomness gets an API of its own, separate from the pseudo-random generator.

## Motivation

Requirement S-1 adds `std.time` and `std.random`. A function that reads the clock or the entropy of the system gives a different result in each run, and the contracts already ask that this be visible: "Clock, random and I/O dependencies of tested applications are explicit injectable interfaces" (T-12, [language-contracts.md](../language-contracts.md) section 5), and a test that uses "an unrecorded file, environment, clock, randomness, network or process result" is not reused (section 6). Without an effect, `unio symbol` would show a function that stamps a record with `time.now()` as pure.

```unio
import std.time as time

function stamp(message as Text) returns Text:
    return "{time.now()} {message}"    # unio symbol: effects: allocates, clock
```

## Design

1. **Derived, without a mandatory permission.** `clock` and `random` are derived like the other effects (semantics 8.2): an external function of `std.time` has `clock`, an external function of `std.random` has `random`, and the rules of 8.4 carry them through calls, interface calls and function values (RFC 0005), so a function that passes `time.now` as a value to a function that calls it has `clock`. They need no permission and are never compared with the manifest (8.3); the manifest of T-6 is unchanged.
2. **`clock` is reading the current time.** Only `now`, `monotonic` and `sleep` (and `SystemClock`, which calls them) have it. Operations on a given duration, instant, timestamp or date have no effect: arithmetic, comparison, the calendar, RFC 3339 text and `ManualClock`. A call of `now()` on a `Clock` value whose type is not known at the call has `clock` by the conservative rule for interfaces (8.4).
3. **`random` is obtaining external randomness**, the entropy of the operating system: `from_entropy`, `secure_bytes` and `secure_int` have it. A pseudo-random `Generator` made with an explicit seed (`random.seeded(n)`) is reproducible and has no effect: its seed and state are explicit, and drawing a number changes the state as the mutation of a `var` value (its methods take `var self`, L-11), which the checker accounts as a mutation, not as an effect.
4. **Tests.** Code that needs the time takes a `Clock`, and a test passes `time.manual_clock(...)`; code that needs pseudo-random numbers takes a `Generator`, and a test passes `random.seeded(n)`. The random source of the system cannot be replaced by a test one: there is no global switch, environment variable or policy key that turns it into a seeded generator, so `from_entropy` and the secure functions are never silently predictable. The only hook, in the runtime crate for its own tests, can make the source fail and nothing else.
5. **Cryptographic randomness is separate.** `std.random.secure_bytes(count)` and `secure_int(low, high)` are the only functions of the library fit for keys and secrets. They read the source of the operating system at every call (`getentropy` on Linux and macOS, `BCryptGenRandom` on Windows), and a system that cannot give randomness fails with `RandomError`; they never fall back to a weaker generator. MRG32k3a, the generator of `Generator`, stays documented as not for cryptography. The planned `std.crypto` (release 0.2) builds on the same source.
6. `unio symbol` prints the effects in the order `files.read`, `files.write`, `network`, `process`, `env`, `allocates`, `clock`, `random`.

## Rationale and limits

- The effects show a dependency; their absence does not prove that a function is deterministic. Its result also depends on its input, on external state it reads (files, the environment, processes, which have effects of their own, but also on what another program changes between two runs), and, once concurrency exists, on scheduling.
- Reading a clock or entropy reveals nothing outside the program and changes nothing, so a permission for every ordinary program is not required now. Precise clocks do matter for timing side channels, so a future strict policy must be able to restrict or virtualize `clock` (and `random`); deriving the effects now is what makes such a policy checkable later. That policy needs an RFC of its own.

## Migration

Not needed: no existing program used `std.time` or `std.random`, and no effect needs a new permission.

## Alternatives

- No effect for the clock and randomness: the two modules would look pure, which hides the very dependency the contracts ask to make explicit.
- Mandatory manifest permissions `time` and `random`: they would change T-6 and every manifest without guarding anything the sandbox needs to guard today; see the rationale for when this changes.
- One effect `nondeterministic` for both: the uses differ (a clock is injected through `Clock`, randomness through a seeded `Generator`), and two names say which one to inject.
- A test switch that seeds the system source: it would make secure randomness predictable without the code saying so.

## Verification

- `compiler/unio/tests/effects.rs`:
  - `fallible_callbacks_carry_their_effects_and_the_clock_and_randomness_are_derived`: `time.now()` gives `clock`, `random.from_entropy()` gives `random`, a manifest without permissions for them gives no E0085;
  - `clock_and_random_follow_function_values_and_skip_pure_operations`: `clock` through a parameter, a lambda and a struct field, `random` through a function value of `secure_int`; no `clock` for the calendar, durations, RFC 3339 text and `ManualClock`, no `random` for a seeded generator and every draw from it;
  - `the_random_source_of_the_system_is_not_replaceable_by_the_environment`: two runs with seed-like variables set draw different numbers.
- `compiler/unio-runtime/src/externs/clock.rs`: `an_unavailable_source_fails_and_never_falls_back` (the test hook makes `secure_bytes` and `secure_int` fail with `RandomError` and `entropy` with a fault), `no_variable_replaces_the_source`, and the range and size of the secure functions.
- The test blocks and examples of `stdlib/std/time.unio` and `stdlib/std/random.unio`.
