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

0002 belongs to common conventions; the platform findings retain their evidence
unchanged under 0008. After `spec-v0`, changes to frozen contracts require an ADR,
matching schemas/fixtures/cases and regenerated consumers. Policy, promotion and
tickets freeze separately at M3.
