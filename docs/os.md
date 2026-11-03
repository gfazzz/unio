# UnioS: a site about Unio, in Unio, on an operating system written in Unio

Version: 0.4 (draft). Changes from 0.3: post-quantum transport, identity, and update requirements (OS-7; `docs/cryptography.md`).

Changes from 0.2: requirements for human and AI understanding, local and remote administration, their acceptance checks, and an integrated base system (sections 1.1–1.2).

Status: accepted as a separate track; not part of release 0.1. Basis: `docs/concept.md` (section 8.1, system programming), `docs/plan.md` (section 9), release 0.5 "System".

## 1. Goal of the proof of concept

A public website about Unio that is:

- written in Unio (content, templates, routing);
- served by an HTTP server and web framework written in Unio;
- running on UnioS, an operating system written in Unio;
- deployed on an ordinary virtual machine of the class "8 GB RAM, 4 CPU cores, 80 GB storage", locally under KVM/QEMU or on a rented VDS, reachable from the internet over HTTPS.

The proof of concept demonstrates the whole stack. It is not a general-purpose operating system: concept section 8.1 states that an operating system with drivers and Linux-level compatibility is a project of many years in which the language is a small part, and that it is not on the roadmap of the language. UnioS is therefore a project that uses the language, with its own specification and team, and this document fixes only what the language must provide for it and how the proof of concept is reached.

### 1.1 Requirements shared with the language

UnioS inherits the language's basic requirements: it must be understandable to people and AI, have explicit behavior, and keep resources under control (concept, sections 1 and 7). These principles apply to system interfaces, configuration, diagnostics, and everyday administration as well as implementation code. A person and an AI agent must be able to discover the available operations, understand their consequences, perform them, and verify the result.

- **OS-1. Understandable interfaces.** Use a small, consistent vocabulary for services, resources, permissions, and operations. Commands and configuration have documented schemas, examples, defaults, and effects. Built-in help and interface discovery expose the same information to a person and an agent; routine administration must not depend on undocumented conventions.
- **OS-2. Management from inside and outside.** Provide a documented CLI and programmatic API for inspecting and managing services, configuration, processes, and resource budgets. An agent running inside UnioS uses local interfaces; an external agent can perform the same authorized operations remotely, including command execution over SSH with standard clients. Automated use requires no GUI, interactive shell, or terminal allocation. Local and remote interfaces share operation semantics and permission checks.
- **OS-3. Structured context and diagnostics.** Alongside readable output, provide versioned machine-readable responses, including JSON: stable object identifiers, state and configuration revisions, error codes, explanations, and suggested remedies. Logs, metrics, service dependencies, and effective permissions are queryable through documented interfaces. Agents must not have to infer state by parsing presentation text.
- **OS-4. Predictable changes.** Support the cycle "inspect, plan, validate, apply, verify". Configuration changes can be validated before application, with their expected effects and required permissions shown. Detect stale revisions; document atomicity and rollback limits. Declare which operations are idempotent; give other operations identifiers so a retry after a lost connection can query the outcome without silently repeating an action. Long operations expose progress, deadlines, and cancellation semantics.
- **OS-5. Explicit authority and accountability.** Local and remote agents act under an authenticated identity with only the capabilities granted to it. SSH authentication does not grant additional system privileges. Record who requested a change, its target, and its outcome in an audit log; expose resource limits and permission denials with actionable diagnostics. Human operators can inspect and revoke an agent's access.
- **OS-6. Acceptance through shared scenarios.** Publish scenarios for inspecting service health and resource use, diagnosing a failed service, validating and applying configuration, restarting a service, and verifying recovery. Run the same scenarios through local interfaces and SSH, both manually and with an AI agent given only the documented interfaces. Check resulting state, error reporting, denied operations, stale revisions, and recovery after a disconnected request. Record completion, human interventions, and failures; the U-0 specification fixes the scenario set and measurable acceptance thresholds before implementation.
- **OS-7. Post-quantum protection by default.** TLS and SSH, identities and trust provisioning, system images and updates, and any encrypted storage or recovery path follow [the cryptography policy](cryptography.md). U-0 specifies the profiles and trust lifecycle; U-3 verifies both key establishment and authentication, update signatures, downgrade rejection, and recovery. Public HTTPS/ACME and standard SSH interoperability may require explicit compatibility endpoints; they are documented separately and do not count as strict-profile acceptance. Update verification never falls back to classical-only signatures.

U-0 specifies these contracts and checks. U-2 delivers local management for the services used by the site. U-3 delivers authenticated SSH management and passes the local and remote scenarios before public deployment. This baseline does not depend on running a model inside UnioS; accelerator support and model serving remain in U-4. A general-purpose interactive shell also remains in U-4; the earlier management CLI must work without it.

### 1.2 A complete, integrated base system

UnioS follows FreeBSD's base-system approach: the kernel, system libraries, essential services, and administrative applications are developed, documented, tested, and released as one coherent system. The project owns their integration and a common set of interfaces and conventions. Installing and operating the supported system must not require users to assemble competing implementations of its essential components.

The base includes the tools needed to boot, install and recover the system; manage storage and files; configure networking, DNS and time; manage identities and permissions; supervise processes and services; inspect logs and resources; administer locally and through SSH; and install, update, back up and restore the system. A command environment, help, and the CLI/API contracts of section 1.1 are part of this base. U-0 defines the inventory, the owner of each component, and the release in which it is required. Each release ships all components needed for its declared scenarios, with compatible versions and end-to-end tests for installation, operation, updates, and recovery.

The first proof of concept supplies the base needed to deploy and administer the site, including a minimal management CLI and a documented update and recovery path. The general-purpose shell, application package manager, and broader user applications follow in U-4. This staged delivery must be stated in release documentation; the site milestone does not establish a complete general-purpose environment.

Additional applications are installed separately from the base through documented packaging and permission interfaces. They may extend or offer alternatives to the supplied tools, while the standard installation remains a supported, consistent default. A coherent base does not by itself provide the breadth of the Linux application ecosystem.

## 2. Why an operating system of its own

What UnioS makes possible that a port to Linux does not:

- **One security model from manifest to hardware.** The scoped package permissions of the concept (`files`, `network`, `process`, `env`) become kernel capabilities. On Linux the sandbox (concept section 6) is built from Landlock, seccomp, Seatbelt, or AppContainer; on UnioS the kernel grants exactly the capabilities the manifest names and nothing else. A program that did not declare `network` has no socket to misuse.
- **No inherited incompatibilities.** No `fork`, signals, `errno`, text-as-bytes ambiguity, or global mutable process state. System calls return `Option` and typed errors exactly as functions do in the language; `Resource` and `using` are the kernel's own ownership model.
- **Unified resource accounting.** Memory budgets, device buffers, and accelerator memory are kernel objects with the same lifetime rules as in the language; the profiler sees RAM, VRAM, and device queues through one event format.
- **AI support in the kernel.** Accelerators (GPU, NPU) as first-class scheduled devices; tensor memory and queues as kernel resources; model serving and agent sandboxes as system services governed by permission manifests; inference latency and memory visible to the scheduler. This is the subject of the later stage U-4 and is measured, not promised.
- **Architectures as capability descriptions.** The kernel targets x86-64 and arm64 with one device model. A quantum processor is not a host architecture: it is a coprocessor driven by classical control code. UnioS represents a QPU as a device with a capability description (qubit count, connectivity, gate set, queue), the same way it represents a GPU. No operating system runs on a QPU, and this document does not claim otherwise.

What UnioS costs:

- Hardware breadth: the proof of concept supports virtio devices only (network, block, console, RNG), which is what KVM, QEMU, and most VDS providers expose. Physical hardware beyond that is out of scope.
- Compatibility: no existing software runs on it. Every component, including TLS, is written in Unio.
- Exposure: a new network stack on the public internet is a target. The safe subset of the language removes memory corruption, but protocol logic must still be fuzzed and reviewed.

## 3. What the language provides

UnioS uses what concept section 8.1 defines and adds nothing to the language:

- **Library layers.** The kernel uses `core` and, with its own allocator, `alloc`. Services and the site use all three layers; on UnioS the `std` layer is implemented over UnioS system calls instead of a host operating system. Application code is written exactly as on Linux.
- **Build profiles.** The kernel is built in the `system` profile (release 0.5): memory without a GC, control of representation, pointers and memory-mapped devices in `unsafe module` files, interrupts, atomic operations, and checked constraints on effects. Services and the site are built in the `application` profile.
- **Sandbox as capabilities.** The sandbox requirements of the specification (SB-1 onward) are satisfied on UnioS by the kernel's capability model; the same manifest yields the same restrictions on Linux and on UnioS.
- **Start layer.** A small start layer in assembly or Rust (bootloader entry, early paging) is acceptable under concept section 1.1, with a stated reason and end.
- **Cryptography for HTTPS and SSH.** Both protocols use reviewed `std.crypto` primitives with constant-time behavior and a cryptographically secure random source (plan section 9.2). Their protocol implementations and interoperability checks are separate; TLS support does not implement SSH. On UnioS the kernel provides entropy from virtio-rng.

Anything UnioS turns out to need beyond this is proposed as an RFC to the language, not implemented as a private fork.

## 4. Architecture of the proof of concept

The design follows concept section 8.1: a small kernel and services in separate processes, so that most components stay in safe Unio and hardware complexity is concentrated in a small part.

```
+-------------------------------------------------------------------+
|  processes (application profile, GC, std over UnioS system calls) |
|    site: content, templates, routing                              |
|    web framework: router, templates, static files, markdown       |
|    HTTP/1.1 server, TLS 1.3 subset                                |
|    services: network (TCP/IP, DNS), storage (file store),         |
|              log, time, config, metrics                           |
|    drivers as services: virtio-net, virtio-blk, virtio-rng        |
+-------------------------------------------------------------------+
|  system calls: typed errors, Option, capabilities, Resource       |
+-------------------------------------------------------------------+
|  kernel (system profile, core + alloc, no GC):                    |
|    boot entry, paging, memory regions, timer, interrupts,         |
|    scheduler, processes, capabilities, virtio transport,          |
|    serial console                                                 |
+-------------------------------------------------------------------+
|  KVM / QEMU / VDS: x86-64 first, arm64 second                     |
+-------------------------------------------------------------------+
```

Design rules:

- The kernel is the only `system` profile component and stays under 15,000 lines; drivers and protocol stacks live in processes.
- Every process is ordinary Unio with `Resource` and typed errors; a driver process holds the device capability and nothing else.
- Scheduling: cooperative tasks on one core in U-1, preemptive across four cores in U-3.
- Isolation is by address space and capabilities from the first user process onward, as in the 0.5 prototype chain.

## 5. Why this VM class is sufficient

| Resource | Proof-of-concept need | Headroom |
|---|---|---|
| RAM 8 GB | kernel under 64 MB; site, TLS, and services under 512 MB | leaves room for an inference service in U-4 |
| 4 cores | 1 core until U-3, then one scheduler per core | |
| 80 GB storage | site, logs, and package cache under 2 GB | leaves room for models and datasets |

## 6. Stages

The track starts together with release 0.5 "System", whose prototype kernel (bootloader, entry into Unio code, serial output, page management, timer, scheduler, user process, system call) is UnioS stage U-1's starting point. It needs native code (plan stage 4), the concurrency model and `std.net` design from 0.2, and the `system` profile from 0.5. Estimates assume a team of 2–3 engineers separate from the language team.

| Stage | Weeks of the track | Result | Exit criterion |
|---|---|---|---|
| U-0 Design | 1–6, during 0.5 | UnioS specification, device model (virtio), capability model mapped from manifest permissions, system-call interface, service protocol | Specification accepted; the 0.5 prototype kernel boots under QEMU with the UnioS system-call table |
| U-1 Kernel | 6–16 | Virtio transport, interrupts, memory regions for processes, process creation and capabilities, serial console, driver processes | A driver process receives interrupts and reads a virtio device; memory and timer tests green under QEMU |
| U-2 Network and site | 14–28 | virtio-net driver, TCP/IP and DNS service, `std` over UnioS system calls, HTTP/1.1 server, web framework, the site over HTTP on a local VM | Site serves from a local VM; the network stack survives 24 h of fuzzing |
| U-3 Storage, TLS, deployment | 26–44 | virtio-blk driver, file store service with the atomic-write guarantees of S-9, TLS 1.3 subset on `std.crypto`, an ACME client in Unio that obtains and renews the certificate, preemptive scheduling on 4 cores, deployment on a VDS | Site reachable from the internet over HTTPS for 30 days with uptime recorded |
| U-4 Beyond the proof of concept | separate project | Shell, package installation, accelerator device model, model-serving service, QPU device model, broader hardware | Own specification, outside the language roadmap |

Stages overlap where the next one can start on finished parts; the weeks are the same as in plan section 9.1. The current draft estimates 44 weeks for U-0 to U-3, which deliver the proof of concept, the UnioS 0.1 milestone, announced on the site it serves. U-0 must reassess these estimates against the base-system inventory and management requirements of sections 1.1–1.2. U-2 acceptance includes local management scenarios; U-3 acceptance also includes the SSH scenarios and the site's update and recovery path before public deployment.

## 7. Risks

| Risk | Likelihood | Mitigation |
|---|---|---|
| UnioS pulls effort from the language before 0.5 is stable | high | Separate team; U-1 starts only when the 0.5 prototype kernel boots |
| Cranelift lacks what the kernel needs | medium | Already covered by the freestanding prototype in plan stage 4 and the LLVM option |
| TLS implemented from scratch has protocol defects | medium | TLS 1.3 with explicitly selected profiles under `docs/cryptography.md`, test vectors from the RFCs, interoperability tests against three existing clients, external review before public exposure |
| `std` over UnioS diverges from `std` over Linux | medium | One `std` interface, two backends; the specification test corpus and the reference utilities run on both |
| Kernel-level AI support stays a slogan | medium | U-4 ships a measurable service: model inference with memory and latency visible in the profiler |
| Expectation that UnioS runs on quantum hardware | medium | The QPU is a coprocessor device model; stated plainly in section 2 |
| Timing side channels in cryptography written in Unio | medium | Constant-time primitives in reviewed `unsafe module` files without allocation or GC; statistical timing tests in CI; external review before public exposure |

## 8. Decision

The track is accepted as "UnioS" and scheduled in the work plan (section 9) alongside release 0.5. Until native code and the `system` profile exist, the only work on this track is design, and every need it discovers goes to the language as an RFC.
