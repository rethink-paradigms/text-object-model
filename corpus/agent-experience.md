# Agent Experience — The Missing Layer

**Date**: 2026-07-10
**Status**: Design gap — not yet implemented

---

## The Core Reframe

The agent is the user. Not the human.

The human writes text and reads text. Both activities are pure flow states — the runtime is invisible to them. But the agent is the primary *consumer* of the runtime's structured representation. Every annotation, every reference, every provenance chain, every transclusion — those interfaces exist for agents, not for humans.

This was not the framing we started with. The original proposal and the storage architecture were designed from the inside out — storage, parsing, identity, annotations. What was never designed is the agent's experience of using the runtime: what it receives when it reads, what it declares when it writes, and how it discovers what exists before a task begins.

---

## The Three Missing Layers

### Layer 1: Discovery — Before the Task Starts

An agent begins a task with a question: *what already exists?*

Current design: none. The agent can call `search(query)` and get matching nodes. But that's reactive — the agent already has to know what to look for.

What agents actually need:
- **Corpus overview**: what documents are in the runtime, when ingested, what formats, what size
- **Annotation landscape**: what's already annotated, by which agents, what types, what confidence
- **Open questions**: what annotations are in `orphan` or `active_partial` state — signals of unresolved uncertainty
- **Relationship graph**: what documents cite, derive from, or contradict which others
- **Topic clusters**: what themes cluster across the corpus (not keyword search — structural)

The runtime has all this data. It just has no discovery interface.

**Prior art**: Wire's "epistemic provenance" — designed for AI agents to consume at inference time, giving each retrieved chunk its typed relationships, source metadata, and confidence state. Not just "here is text" but "here is text + its epistemic context."

---

### Layer 2: Structured Read Brief — During Reading

When the runtime projects a document to an agent, the current design sends:
- §N-marked text (sentence numbers as cursor)

What an agent actually needs to read productively:
```
Document: "analysis-of-compression.md"
Ingested: 2026-07-08, v3 (2 re-ingestions since original)
Annotations: 7 total — 4 active, 2 active_partial, 1 orphan
  §3: [highlight by intel-researcher: "key claim"] confidence 1.0
  §14: [comment by web-researcher: "contradicted by source B"] confidence 0.9
  §§22-24: [tag: "open-question"] confidence 0.8
Prior references: this document is cited by 3 others
Derived from: 2 source documents (transclusion edges: cites, derives-from)

§1 The core problem with context compression is...
§2 ...
§3 Retrieval-augmented generation solves half the problem...
```

The agent reads text AND its full epistemic state simultaneously. It knows which sentences are contested, which are well-anchored, which other agents have flagged things. This is what makes agent-to-agent communication through the runtime possible.

---

### Layer 3: Output Declaration — After Writing

When an agent writes a document, each sentence has a relationship to source material. Currently: nothing captures this. The output goes to disk as clean Markdown, and the epistemic chain is lost.

What needs to exist: a lightweight interface for agents to declare output provenance.

**TROVE relationship types** (ACL 2025) — the right vocabulary:
- `quotation` — exact or near-exact reproduction
- `compression` — summary or distillation
- `inference` — conclusion drawn from source
- `composition` — synthesized from multiple sources
- `original` — no source (genuinely new content)

**The interface** (not yet designed):
```
declare_output({
  document: "my-output.md",
  sentences: [
    { sentence: 1, type: "compression", source_uuid: "...", source_sentence: 14 },
    { sentence: 2, type: "inference", sources: ["...", "..."] },
    { sentence: 3, type: "original" }
  ]
})
```

This turns agent output from an opaque Markdown file into a verifiable, traceable artifact. The runtime stores these declarations as provenance annotations. Future agents reading the output can see exactly where each claim came from.

**Why this matters**: Most "hallucinations" in retrieval-grounded systems are unfaithful attributions — the agent cites a source that does not support the claim. Output declaration, even partial, is a structural check on this.

---

## The UUID Migration / Intent Problem

When an agent transforms text from document A into document B — summarizes it, rewrites it, translates it — what happens to the UUID relationship?

Three cases:
1. **Transclusion** — live reference, changes propagate. UUID stays linked.
2. **Derivation** — independent copy, but provenance recorded. New UUID, but edge to source.
3. **Original** — no source relationship. New UUID, no edge.

In the current design, the agent has to explicitly declare which case applies. This is friction. Agents transform text constantly. Requiring explicit declarations for every transformation breaks the writing flow — the same problem the runtime solves for humans.

**The design question not yet answered**: Can the runtime infer intent from context? Or is explicit declaration always required? If explicit, what's the minimum viable declaration that doesn't burden the agent?

---

## What the Research Said That We Missed

Reading the three research outputs with the agent-as-user lens:

**Wire epistemic provenance** (web-researcher Finding 4):
> "Designed for AI agents to consume at inference time — influences the agent's next action. Most hallucinations in retrieval-grounded systems are actually unfaithful attributions."

**TROVE** (ACL 2025, web-researcher sources):
Sentence-level tracing of AI output back to source sentences. Relationship classification at sentence granularity. This is the academic validation that output declaration is tractable.

**intel-researcher's "AI-native" requirement**:
Listed as one bullet but never designed: "designed for agents to query and compose using UUIDs." The §N interface addresses query. Compose — how agents build new documents from existing ones with declared provenance — was never designed.

**The lego-researcher's BendScript typed predicates**:
`transcludes`, `derives-from`, `cites`, `contradicts`, `responds-to`, `supports`, `supersedes`, `exemplifies` — these are exactly the output declaration relationship types. They're already in the transclusion data model. They need to be surfaced as an agent-facing interface, not just a storage primitive.

---

## Summary: What's Built vs. What's Missing

| Layer | Status |
|---|---|
| Parse, segment, UUID assign, store | ✅ Built (Rust binary, 10+ formats tested) |
| §N agent cursor | ✅ Designed (storage-architecture.md Section 11) |
| W3C annotation storage | ✅ Built |
| Provenance data model | ✅ Built |
| **Discovery interface** | ❌ Not designed |
| **Structured read brief** | ❌ Not designed |
| **Output declaration interface** | ❌ Not designed |
| **UUID migration / intent protocol** | ❌ Open question |
| Pi SDK tool wrapper | ❌ Not built |
