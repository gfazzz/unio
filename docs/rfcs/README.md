# Unio RFCs

Russian localization: [docs/ru/rfcs/README.md](../ru/rfcs/README.md).

An RFC (request for comments) is how the syntax, the semantics, the keyword set, or a normative requirement changes after stage 1 of the [plan](../plan.md). Before that, changes are made directly in the concept, the requirements, and `spec/`, with a "Changes from" note at the top of the changed document.

## Process

1. Copy [0000-template.md](0000-template.md) to `NNNN-short-title.md` with the next free number.
2. Open a pull request with status `proposed`. Discussion happens in the pull request.
3. The language lead accepts or rejects the RFC. An accepted RFC gets status `accepted`; a rejected one stays in the repository with status `rejected` and the reason.
4. The same pull request, or the next one, updates `docs/concept.md`, `docs/requirements.md`, `spec/grammar.ebnf`, the corpus in `spec/tests/`, and the Russian localization. An RFC is `implemented` when the compiler supports it.

## Rules

- An RFC that adds a keyword or a construct must answer, in its own section, why existing constructs cannot express the change.
- An incompatible change ships with an automatic migration in `unio fmt --migrate` or with a diagnostic that carries a fix (requirement R-9).
- A grammar change comes with valid and invalid corpus files that pass `spec/check_grammar.py --corpus`.

## Index

| Number | Title | Status |
|---|---|---|
| — | No RFCs yet | — |
