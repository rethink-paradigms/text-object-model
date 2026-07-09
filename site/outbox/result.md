## Result: Multi-page HTML site presenting Text Runtime research
status: done

## Changes made

- `workspace/text-object-model/site/index.html` — Landing page with overview, card grid linking to 6 dimensions + synthesis. Full design system matching reference site.
- `workspace/text-object-model/site/the-pattern.html` — The convergent 4-layer architecture pattern discovered across 60 years by Xanadu, Purple Numbers, SiSU, Hypothesis, Notion, and BendScript.
- `workspace/text-object-model/site/stable-identity.html` — Stable node identity: UUID v7, HIDs vs NIDs, CRDTs, sentence-level granularity, UUID assignment patterns (SuperDoc paraId).
- `workspace/text-object-model/site/annotations.html` — Annotation overlays and anchoring: W3C Web Annotation model, Hypothesis 4-strategy cascade, dual selectors (Semiont), LFCC state machine, standoff markup.
- `workspace/text-object-model/site/transclusion.html` — Transclusion and provenance: Obsidian/Magma WikiLinks, org-transclusion live-sync, BendScript typed edges, C2PA/Encypher/Wire/PROV-O provenance, the transclusion edit problem.
- `workspace/text-object-model/site/architecture.html` — Architecture and pipeline: 5-stage pipeline, core data structures (TextNode, Annotation), 8 key decisions, 10 lego pieces from prior art.
- `workspace/text-object-model/site/hard-problems.html` — Five hard problems (granularity, transclusion edit, UUID migration, re-anchoring failure UX, annotation ownership), 7 open questions, why prior systems failed.
- `workspace/text-object-model/site/synthesis.html` — Complete synthesis: 6-dimension recap, 15-pair interaction map, 5 cross-cutting patterns, 4 core tensions, 3 things that would change everything.

## Design

- Design system mirrors the reference site (`navigation-architecture-site`) — Inter font, soft card-based layouts, colored dimension stripes, sticky top navigation, cross-reference grids, pullout quotes.
- 6 distinct color themes (one per dimension) + gold synthesis theme.
- Every page has: sticky top nav, page header with badge/question, sectioned content, cross-references footer linking to related dimensions.
- Typography: Inter, -apple-system fallback, -webkit-font-smoothing, letter-spacing -0.03em for headings.

## How to verify

```bash
# Open the landing page
open workspace/text-object-model/site/index.html

# Navigate through all 8 pages, verify:
# - All nav links work (index ↔ dimension pages ↔ synthesis)
# - All cross-reference cards link to correct pages
# - Responsive at mobile widths (<600px, <768px breakpoints)
# - No broken styling
```

## Content sources synthesized

- 3 agent outputs (intel-researcher, web-researcher, lego-researcher)
- 5 research files (index.md, understanding.md, findings.json + 3 source files)
- 5 lego implementations
- 3 design documents (proposal.md, decisions.md, sources.md)
- 1 original proposal (workspace/text-object-model/proposal.md)
- Reference site: 7 HTML files for design/typography/structural patterns

## Notes

- 167KB total across 8 files (16KB–27KB each)
- All 56 cross-page links verified consistent
- No external dependencies — pure HTML/CSS, self-contained
- The site is optimized for human comprehension: each page answers a clear question, uses progressive disclosure, and connects to related dimensions
