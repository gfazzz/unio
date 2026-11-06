# Reference customer: tanstaafl

Version 0.1-draft. Russian localization: [ru/customers.md](ru/customers.md).

A language meant for writing products with AI is checked by writing a product with AI. tanstaafl is that product: a B2B platform for building security systems (fire alarm, voice evacuation, access control, video surveillance, structured cabling, building dispatch) that audits a site against the norms with AI, generates designs, monitors sites around the clock with prediction, and has a separate post-quantum module. It is written by AI under human review, which is the use Unio is designed for.

Today tanstaafl has a Rails portal; its AI service (Python), site agent (Rust with protocol drivers in C), event core (Elixir/OTP), and integrations (TypeScript) are planned. Each part becomes a scenario here. A scenario either runs in a named release of Unio or names what that release lacks.

## 1. Rules

- A scenario program lives in [../examples](../examples), uses only the standard library in [../stdlib](../stdlib), and follows the rules of the reference utilities: its usage and exit codes in the documentation comment, `test` blocks written before the code. CI checks its syntax and API use; its tests run once the stage 3 interpreter works (plan, section 3).
- A gap is recorded in section 4 with its target: the existing requirement it waits for, a release, or an RFC. Closing a gap changes the concept, the requirements, or the library by the usual process (concept section 1.2, [rfcs/README.md](rfcs/README.md)); this document only points to where it landed.
- The customer adds no requirement by itself. A gap becomes a requirement when the release it targets is specified.

## 2. Principles of the customer and what in Unio holds them

| Principle of tanstaafl | What it asks of the language | Where Unio has it |
|---|---|---|
| A person in the loop: AI prepares drafts and findings, a person approves significant actions and signs them | Approval is part of the data; the signature is checkable | Scenario A keeps unapproved findings out of the totals; signatures wait for RC-15 |
| Risk is money: R = P × C, ALE and ROSI; every audit finding cites a clause of a norm | Exact arithmetic of money; overflow is not silent | `Int` overflow is a fault and `checked_*` are its safe forms (concept, section 3.3); exact money waits for `std.decimal` (S-1), see RC-2 |
| Site data are data, not instructions; an agent has the rights of an intern of unknown loyalty | Least privilege of a program; untrusted text cannot become a command | Manifest permissions and the sandbox (T-6, SB-1); typed untrusted text is RC-14 |
| The site lives without the cloud for 72 hours; the cloud aggregates | Writes that survive a power cut; a service that runs for days | Atomic and durable writes (S-9) in scenario B; long-running services are RC-10 |
| Zero trust and crypto agility: mTLS, the algorithm is a replaceable parameter | Hybrid post-quantum key exchange, versioned profiles | [cryptography.md](cryptography.md) |

## 3. Scenarios

| ID | Scenario | Part of tanstaafl | Release of Unio | State |
|---|---|---|---|---|
| A | Risk register: audit findings with norm references ranked by annual loss, ROSI of each fix, drafts kept apart from approved findings | Audit module | 0.1 | [../examples/risk_register.unio](../examples/risk_register.unio) |
| B | Event journal of a site agent: undelivered events kept however old, delivered ones removed after the 72-hour window, a warning when the window is exceeded | Site agent | 0.1 | [../examples/event_journal.unio](../examples/event_journal.unio) |
| C | Field protocol frames: a Modbus RTU response read from bytes and checked by its CRC-16 | Site agent, protocol drivers | 0.1 for the frames, 0.2 for the ports | Waits for RC-7, RC-8, RC-9 |
| D | Event core: a service per site that receives events, keeps a state machine of each alarm, and is restarted when it fails | Event core | 0.2 | Waits for RC-10, RC-11 |
| E | Portal: pages in two languages, sign-in for the team, a site dashboard over PostgreSQL | Portal | 0.2 | Waits for RC-12, RC-13 |
| F | AI service: a norm question answered by an LLM with citations; site documents never reach the model as instructions | AI service | 0.2 | Waits for RC-14 and the HTTPS client of 0.2 |
| G | Post-quantum channel between a site and the cloud: mTLS with hybrid key exchange, the algorithm chosen by a profile | PQC module | 0.2 | Follows [cryptography.md](cryptography.md) |

Scenarios A and B fit the scope of 0.1: command-line programs over files and JSON. Writing them found RC-1 to RC-6.

## 4. Gaps

| ID | Gap | Found by | Why the customer needs it | Target |
|---|---|---|---|---|
| RC-1 | No conversion from `Float` to `Int`: no `round`, `floor`, or `to_int` with an `Option` result | A | A frequency of 0.05 incidents a year cannot be turned into money; scenario A counts incidents per hundred years in `Int` instead | 0.1 library, `stdlib/core/builtin/float.unio`; no RFC |
| RC-2 | No exact decimal money | A | Kopecks, VAT, and rates need decimal arithmetic with a named rounding mode; scenario A uses whole rubles | `std.decimal` of S-1, stage 5; scenario A moves to it |
| RC-3 | No `sum` of a `List[Int]` and no checked sum | A | Totals of a register are written by hand as loops | 0.1 library, `stdlib/alloc/builtin/list.unio` |
| RC-4 | No clock | B | Scenario B takes NOW as an argument | `std.time` of S-1, stage 5 |
| RC-5 | No durable appending to a file | B | Each acknowledgement rewrites the whole journal; on the flash card of a site controller this costs time and wear. Needed: `files.open_append(path, durable)` with a stated rule for a torn last line after a power cut | Refinement of S-9 (a requirement with a letter suffix next to it) |
| RC-6 | No lock that keeps a second writer out of a file | B | Two copies of the agent would each rewrite the journal, and the last rename would lose the other's events | Refinement of S-9 next to RC-5 |
| RC-7 | `Bytes` has only its length and `to_text`: no index, slice, construction from numbers, or reading of 16- and 32-bit numbers in a byte order | C | Every field protocol is a byte frame | 0.1 library, `stdlib/alloc/builtin/bytes.unio` |
| RC-8 | No bit operations on `Int` | C | CRC-16 of Modbus and the flags of control panels are bit work | Methods such as `bit_and`, `bit_xor`, `shift_left` in `stdlib/core/builtin/int.unio` need no RFC and keep the vocabulary; operators would need one |
| RC-9 | No serial ports and no TCP | C | Panels are reached over RS-485 and Modbus TCP | 0.2: `std.net`; a serial port module to be specified with it |
| RC-10 | No long-running service: no wait, no timer, no orderly stop on a termination signal | B, D | The agent and the event core run for days and must stop without losing a write | 0.2, tasks |
| RC-11 | No supervision: a failed task is not restarted by a stated strategy | D | The event core is planned in OTP for exactly this | RFC for 0.2, together with tasks |
| RC-12 | No database access, and FFI is closed to user code (requirements, section 2.2) | E | The portal keeps its data in PostgreSQL | 0.2: the PostgreSQL protocol written in Unio over `std.net`, with SCRAM from `std.crypto`; to be decided when 0.2 is specified |
| RC-13 | No HTTP server, routing, templates with escaping, sessions, or localization | E | The whole portal | 0.2 and the web framework of UniOS stage U-2 ([os.md](os.md)); one framework for both |
| RC-14 | Untrusted text has the same type as any other text | F, E | Text of a site document must not reach an LLM prompt, a shell, SQL, or HTML without an explicit, visible conversion | RFC; candidate for 0.2 |
| RC-15 | No signatures | A | The approval of a finding is a name today; it has to be a signature a third party can check | 0.2: `std.crypto` with the signatures of [cryptography.md](cryptography.md) |

RC-1, RC-3, RC-7, and RC-8 are library additions within 0.1 and can be made now, with tests, in the layers they name. RC-5 and RC-6 change S-9 and go through the requirements. The rest belong to the specification of 0.2.

## 5. What has been checked

Both programs parse with [../spec/grammar.ebnf](../spec/grammar.ebnf) and call only the declared standard library (`spec/check_grammar.py`, `spec/check_api.py`). Their tests have not run: there is no interpreter yet.
