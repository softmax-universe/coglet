# Architecture decision records

Status is the current decision state, not evidence of product qualification.
The spec-v0 freeze is accepted under ADR-0009; implementation qualification
and the other ADR status decisions remain separate.

| Number | Title | Status |
|---|---|---|
| 0000 | [Template](0000-template.md) | Template (not a decision) |
| 0001 | [WAMR caps, interruption, wakes and memory](0001-engine-findings-cap.md) | Proposed |
| 0002 | [Common wire conventions](0002-common-conventions.md) | Proposed |
| 0003 | [Determinism rules](0003-determinism-rules.md) | Proposed; R4 amended by freeze proposal; transform qualification pending |
| 0004 | [Hub control and relay protocol](0004-hub-protocol.md) | Proposed |
| 0005 | Policy | Reserved; outside spec-v0 (M3) |
| 0006 | [Core Wasm ABI and atomic step journals](0006-coglet-abi.md) | Proposed |
| 0007 | [Surface transport and portable speech profiles](0007-surface-speech.md) | Proposed |
| 0008 | [WAMR platform findings](0008-engine-findings-platforms.md) | Proposed; renumbered from duplicate 0002 |
| 0009 | [Spec v0 freeze](0009-spec-v0-freeze.md) | Accepted; implementation qualification excluded |
| 0010 | [Heard-audio truncation and speech work handoff](0010-speech-heard-truncation-and-work-handoff.md) | Proposed |
| 0011 | Design-document fold-in | Reserved by orchestrator |
| 0012 | Hub publish-time float rewrite (if ADR-0003 changes) | Reserved by orchestrator |
| 0013 | [Factory database capability and recovery boundary](0013-factory-db-capability.md) | Proposed; PR #41 |
| 0014 | [Isolated M1 echo fixture catalog](0014-m1-echo-fixture-catalog.md) | Accepted; PR #73; implementation qualification separate |
| 0015 | [Executable companion fixtures for archive-profile replay](0015-archive-executable-guest-fixtures.md) | Proposed |
| 0017 | [Recorded game observations and motor-only game extensions](0017-game-observations-and-research.md) | Proposed |
| 0020 | [Vocabulary: router, adapter, runtime](0020-vocabulary.md) | Proposed; stage 1 (prose) only |
| 0022 | [Embedded product catalogs and recorded pod command admission](0022-embedded-product-catalogs.md) | Proposed |
| 0027 | [Revalidate and resume running coglets after daemon restart](0027-daemon-restart-resume.md) | Proposed; implementation qualification separate |
| 0031 | [Manage port, and patches on a plain filesystem](0031-manage-port-and-filesystem-patches.md) | Accepted decision (#157); hub 1.0 wire effective on reviewed merge of #164; implementation qualification separate |
| 0033 | [Runtime policy attestations and emergency output custody](0033-runtime-policy-producers.md) | Proposed; docs-first producer contract, implementation qualification separate |
| 0036 | [A cogent has no centre; it is present on a few coglets](0036-distributed-cogent-and-presence.md) | Proposed; direction decided by David 2026-11-25, presence messages not yet in the spec |
| 0034 | [Ready-output retrieval before optional embedded runtime pumping](0034-ready-output-service.md) | Proposed; implementation qualification separate |
| 0035 | [State hash is the git SHA-256 tree id](0035-state-hash-git-tree-id.md) | Proposed; direction decided by David 2026-11-24 (unforged staging), implementation separate |
| 0037 | [Baby Bear iOS command disposition for the embedded cutover](0037-babybear-ios-command-disposition.md) | Proposed; status admission only, physical qualification separate |
| 0038 | [Private observation ownership and transfer fences](0038-private-observation-ownership.md) | Accepted decision (#204); implementation qualification separate |
| 0039 | [Fail-closed iOS process-scoped runtime clock](0039-ios-process-runtime-clock.md) | Accepted decision (#207); renumbered from duplicate 0038; implementation and physical qualification separate |
| 0039 | [Coglets live in unforged repos; hubs and smart routers](0039-hubs-and-coglets-on-unforged.md) | Proposed; direction decided by David 2026-11-25, target design, implementation staged |

0002 belongs to common conventions; the platform findings retain their evidence
unchanged under 0008. After `spec-v0`, changes to frozen contracts require an ADR,
matching schemas/fixtures/cases and regenerated consumers. Policy, promotion and
tickets freeze separately at M3.
